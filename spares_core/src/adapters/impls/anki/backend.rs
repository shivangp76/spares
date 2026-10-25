//! Reading and writing Anki's notes for the notes repository's `anki` branch (see
//! [`crate::notes_vcs::anki`]). Only notes of the note type spares writes are in scope: `Basic`,
//! with a `SparesId` field. Notes of other note types are neither read nor written.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use super::ANKI_ADAPTER_NAME;
use super::AnkiAdapter;
use super::AnkiNote;
use super::api::execute_request;
use super::api::execute_requests;
use super::database::anki_note_from_fields;
use super::database::read_database_file;
use super::database::read_spares_model_notes;
use super::to_anki_fields;
use super::types::ApiAction;
use super::types::ApiRequest;
use super::types::ApiRequestParams;
use super::types::FindNotesApiRequestData;
use super::types::ModelName;
use super::types::NoParams;
use super::types::NoteFields;
use super::types::NoteInfo;
use super::types::NotesInfoApiRequestData;
use super::types::UpdateNoteApiRequestData;
use super::types::UpdateNoteApiRequestNoteData;
use crate::AdapterErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::model::NoteId;
use crate::parsers::NoteImportAction;
use crate::parsers::NoteSettings;
use crate::parsers::Parseable;

/// A note as Anki stores it.
#[derive(Clone, Debug, PartialEq)]
pub struct AnkiRawNote {
    pub id: i64,
    /// The fields in the note type's order, separated by U+001F: front, back, keywords,
    /// `SparesId`, `SparesParserName`
    pub flds: String,
    /// Separated by spaces
    pub tags: String,
}

impl AnkiRawNote {
    /// The spares note it describes
    pub fn to_note(&self) -> AnkiNote {
        anki_note_from_fields(self.id, &self.flds, &self.tags, false, None)
    }
}

/// Anki, as the `anki` branch reads and writes it.
#[async_trait]
pub trait AnkiBackend: Send + Sync {
    /// Anki's notes of the note type spares writes, oldest first.
    async fn read_notes(&self) -> Result<Vec<AnkiRawNote>, Error>;

    /// Adds, updates and deletes notes, as [`AnkiAdapter::apply_notes`] does: the ids of
    /// [`NoteImportAction::Update`] and [`NoteImportAction::Delete`] are Anki's, and an added note's
    /// `SparesId` is `custom_data[NOTE_ID_KEY]`. Returns the position in `notes` and the new Anki id
    /// of each added note.
    ///
    /// Anki has no transactions, so after an error some of the changes may have been made.
    async fn apply(
        &self,
        parser: &dyn Parseable,
        notes: Vec<(NoteSettings, Option<String>)>,
    ) -> Result<Vec<(usize, i64)>, Error>;

    /// Sets the `SparesId` field of Anki notes, given as `(Anki id, spares id)`.
    async fn set_spares_ids(&self, ids: &[(i64, NoteId)]) -> Result<(), Error>;
}

fn anki_error(error: impl Into<String>) -> Error {
    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
        adapter_name: ANKI_ADAPTER_NAME.to_string(),
        error: error.into(),
    }))
}

/// Whether `AnkiConnect` answers, which means Anki is open.
pub async fn anki_connect_is_up() -> bool {
    let Ok(client) = Client::builder().timeout(Duration::from_secs(3)).build() else {
        return false;
    };
    let request = ApiRequest {
        action: ApiAction::Version,
        params: ApiRequestParams::None(NoParams {}),
        version: 6,
    };
    execute_request(&request, &client).await.is_ok()
}

/// Anki through `AnkiConnect`, if Anki is open. Otherwise its database file at `ANKI_DB_PATH`, which
/// can only be read.
pub async fn connect() -> Result<Box<dyn AnkiBackend>, Error> {
    if anki_connect_is_up().await {
        return Ok(Box::new(AnkiConnect));
    }
    match std::env::var_os("ANKI_DB_PATH") {
        Some(path) => Ok(Box::new(AnkiFile {
            path: PathBuf::from(path),
        })),
        None => Err(anki_error(
            "Anki is not open (AnkiConnect does not answer at http://localhost:8765), and ANKI_DB_PATH is not set to read its collection instead",
        )),
    }
}

/// Anki while it is open, through the `AnkiConnect` add-on. Reading through it, rather than from a
/// copy of the database, sees Anki's latest changes, which it may not have written to the file
/// yet.
pub struct AnkiConnect;

#[async_trait]
impl AnkiBackend for AnkiConnect {
    async fn read_notes(&self) -> Result<Vec<AnkiRawNote>, Error> {
        let client = Client::new();
        let request = ApiRequest {
            action: ApiAction::FindNotes,
            params: ApiRequestParams::FindNotes(FindNotesApiRequestData {
                query: "note:Basic".to_string(),
            }),
            version: 6,
        };
        let ids: Vec<i64> = serde_json::from_value(execute_request(&request, &client).await?)
            .map_err(|e| anki_error(format!("Unexpected findNotes result: {e}")))?;
        let mut notes = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(1000) {
            let request = ApiRequest {
                action: ApiAction::NotesInfo,
                params: ApiRequestParams::NotesInfo(NotesInfoApiRequestData {
                    notes: chunk.to_vec(),
                }),
                version: 6,
            };
            let infos: Vec<Value> =
                serde_json::from_value(execute_request(&request, &client).await?)
                    .map_err(|e| anki_error(format!("Unexpected notesInfo result: {e}")))?;
            for info in infos {
                // A note deleted since `findNotes` comes back as `{}`
                let Ok(info) = serde_json::from_value::<NoteInfo>(info) else {
                    continue;
                };
                if info.model_name != "Basic" || !info.fields.contains_key("SparesId") {
                    continue;
                }
                let mut fields: Vec<_> = info.fields.into_values().collect();
                fields.sort_by_key(|field| field.order);
                notes.push(AnkiRawNote {
                    id: info.note_id,
                    flds: fields
                        .into_iter()
                        .map(|field| field.value)
                        .collect::<Vec<_>>()
                        .join("\u{1f}"),
                    tags: info.tags.join(" "),
                });
            }
        }
        notes.sort_by_key(|note| note.id);
        Ok(notes)
    }

    async fn apply(
        &self,
        parser: &dyn Parseable,
        notes: Vec<(NoteSettings, Option<String>)>,
    ) -> Result<Vec<(usize, i64)>, Error> {
        AnkiAdapter::without_prompt()
            .apply_notes(notes, parser, false, true)
            .await
    }

    async fn set_spares_ids(&self, ids: &[(i64, NoteId)]) -> Result<(), Error> {
        let requests: Vec<ApiRequest> = ids
            .iter()
            .map(|(anki_id, spares_id)| ApiRequest {
                action: ApiAction::UpdateNote,
                params: ApiRequestParams::UpdateNote(UpdateNoteApiRequestData {
                    note: UpdateNoteApiRequestNoteData {
                        deck_name: "Default".to_string(),
                        model_name: ModelName::Basic,
                        id: *anki_id,
                        fields: NoteFields {
                            front: None,
                            back: None,
                            keywords: None,
                            spares_id: Some(spares_id.letters()),
                            spares_parser_name: None,
                        },
                        tags: None,
                    },
                }),
                version: 6,
            })
            .collect();
        execute_requests(&requests, false, true, &Client::new()).await?;
        Ok(())
    }
}

/// Anki's collection file, read from a copy while Anki is closed. Writing needs Anki open.
pub struct AnkiFile {
    pub path: PathBuf,
}

#[async_trait]
impl AnkiBackend for AnkiFile {
    async fn read_notes(&self) -> Result<Vec<AnkiRawNote>, Error> {
        let copy = read_database_file(&self.path).await?;
        Ok(read_spares_model_notes(&copy.pool)
            .await?
            .into_iter()
            .map(|row| AnkiRawNote {
                id: row.id,
                flds: row.flds,
                tags: row.tags,
            })
            .collect())
    }

    async fn apply(
        &self,
        _parser: &dyn Parseable,
        _notes: Vec<(NoteSettings, Option<String>)>,
    ) -> Result<Vec<(usize, i64)>, Error> {
        Err(anki_error(
            "Anki must be open, with AnkiConnect, to change its notes",
        ))
    }

    async fn set_spares_ids(&self, _ids: &[(i64, NoteId)]) -> Result<(), Error> {
        Err(anki_error(
            "Anki must be open, with AnkiConnect, to change its notes",
        ))
    }
}

/// A change [`FakeAnki`] was asked to make
#[derive(Clone, Debug, PartialEq)]
pub enum FakeAnkiCall {
    Add {
        anki_id: i64,
        spares_id: Option<String>,
    },
    Update {
        anki_id: i64,
        spares_id: Option<String>,
    },
    Delete {
        anki_id: i64,
    },
    SetSparesId {
        anki_id: i64,
        spares_id: NoteId,
    },
}

#[derive(Debug, Default)]
struct FakeAnkiState {
    notes: BTreeMap<i64, AnkiRawNote>,
    next_id: i64,
    calls: Vec<FakeAnkiCall>,
    fail_after: Option<usize>,
}

/// An Anki in memory, for tests. It stores notes as `AnkiConnect` would, with tags sorted.
#[derive(Debug, Default)]
pub struct FakeAnki {
    state: Mutex<FakeAnkiState>,
}

impl FakeAnki {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeAnkiState {
                next_id: 1_700_000_000_000,
                ..FakeAnkiState::default()
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeAnkiState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds or replaces a note, as if made in Anki
    pub fn put(&self, note: AnkiRawNote) {
        let mut state = self.state();
        state.next_id = state.next_id.max(note.id + 1);
        state.notes.insert(note.id, note);
    }

    /// Removes a note, as if deleted in Anki
    pub fn remove(&self, id: i64) {
        self.state().notes.remove(&id);
    }

    pub fn note(&self, id: i64) -> Option<AnkiRawNote> {
        self.state().notes.get(&id).cloned()
    }

    pub fn notes(&self) -> Vec<AnkiRawNote> {
        self.state().notes.values().cloned().collect()
    }

    /// The changes asked for since the last call
    pub fn take_calls(&self) -> Vec<FakeAnkiCall> {
        std::mem::take(&mut self.state().calls)
    }

    /// Makes [`AnkiBackend::apply`] fail after `changes` more changes, as Anki can midway.
    pub fn fail_after(&self, changes: usize) {
        self.state().fail_after = Some(changes);
    }

    /// The fields and tags spares writes for a note, as Anki stores them
    pub fn stored(
        parser: &dyn Parseable,
        settings: &NoteSettings,
        data: &str,
    ) -> Result<(String, String), Error> {
        let fields = to_anki_fields(parser, settings, data)?;
        let mut tags: Vec<String> = fields
            .tags
            .iter()
            .flat_map(|tag| tag.split(' '))
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect();
        tags.sort();
        tags.dedup();
        Ok((fields.joined(), tags.join(" ")))
    }
}

#[async_trait]
impl AnkiBackend for FakeAnki {
    async fn read_notes(&self) -> Result<Vec<AnkiRawNote>, Error> {
        Ok(self.notes())
    }

    async fn apply(
        &self,
        parser: &dyn Parseable,
        notes: Vec<(NoteSettings, Option<String>)>,
    ) -> Result<Vec<(usize, i64)>, Error> {
        let mut added = Vec::new();
        for (i, (settings, data)) in notes.into_iter().enumerate() {
            let Some(data) = data else { continue };
            let mut state = self.state();
            if let Some(remaining) = state.fail_after.as_mut() {
                if *remaining == 0 {
                    state.fail_after = None;
                    return Err(anki_error("the fake Anki failed"));
                }
                *remaining -= 1;
            }
            let spares_id = to_anki_fields(parser, &settings, &data)?.spares_id;
            match settings.action {
                NoteImportAction::Add => {
                    let (flds, tags) = Self::stored(parser, &settings, &data)?;
                    let anki_id = state.next_id;
                    state.next_id += 1;
                    state.notes.insert(
                        anki_id,
                        AnkiRawNote {
                            id: anki_id,
                            flds,
                            tags,
                        },
                    );
                    added.push((i, anki_id));
                    state.calls.push(FakeAnkiCall::Add { anki_id, spares_id });
                }
                NoteImportAction::Update(id) => {
                    let anki_id = id.packed();
                    let (flds, tags) = Self::stored(parser, &settings, &data)?;
                    let Some(note) = state.notes.get_mut(&anki_id) else {
                        return Err(anki_error(format!("Anki has no note {anki_id}")));
                    };
                    note.flds = flds;
                    note.tags = tags;
                    state
                        .calls
                        .push(FakeAnkiCall::Update { anki_id, spares_id });
                }
                NoteImportAction::Delete(id) => {
                    let anki_id = id.packed();
                    state.notes.remove(&anki_id);
                    state.calls.push(FakeAnkiCall::Delete { anki_id });
                }
            }
        }
        Ok(added)
    }

    async fn set_spares_ids(&self, ids: &[(i64, NoteId)]) -> Result<(), Error> {
        let mut state = self.state();
        for (anki_id, spares_id) in ids {
            if let Some(note) = state.notes.get_mut(anki_id) {
                let mut fields: Vec<String> =
                    note.flds.split('\u{1f}').map(str::to_string).collect();
                fields.resize(fields.len().max(5), String::new());
                fields[3] = spares_id.letters();
                note.flds = fields.join("\u{1f}");
            }
            state.calls.push(FakeAnkiCall::SetSparesId {
                anki_id: *anki_id,
                spares_id: *spares_id,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use super::AnkiBackend;
    use super::AnkiFile;

    async fn collection(name: &str, schema: &[&str]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "spares-anki-test-{}-{name}.anki2",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let pool = SqlitePool::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for statement in schema {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool.close().await;
        path
    }

    const NOTES: [&str; 2] = [
        "CREATE TABLE notes (id INTEGER, mid INTEGER, flds TEXT, tags TEXT)",
        "INSERT INTO notes VALUES (10, 1, 'front\u{1f}back\u{1f}\u{1f}gir\u{1f}markdown', ' a b '), (11, 2, 'cloze\u{1f}extra', '')",
    ];

    /// Notes of other note types than spares' `Basic` with a `SparesId` field are not read.
    #[tokio::test]
    async fn test_file_reads_only_spares_note_type() {
        let tables = collection(
            "tables",
            &[
                "CREATE TABLE notetypes (id INTEGER, name TEXT)",
                "CREATE TABLE fields (ntid INTEGER, ord INTEGER, name TEXT)",
                "INSERT INTO notetypes VALUES (1, 'Basic'), (2, 'Cloze')",
                "INSERT INTO fields VALUES (1, 0, 'Front'), (1, 3, 'SparesId'), (2, 0, 'Text')",
                NOTES[0],
                NOTES[1],
            ],
        )
        .await;
        // Before Anki 2.1.28, note types were JSON in `col.models`.
        let json = collection(
            "json",
            &[
                "CREATE TABLE col (models TEXT)",
                r#"INSERT INTO col VALUES ('{"1": {"name": "Basic", "flds": [{"name": "Front"}, {"name": "SparesId"}]}, "2": {"name": "Cloze", "flds": [{"name": "Text"}]}}')"#,
                NOTES[0],
                NOTES[1],
            ],
        )
        .await;
        for path in [tables, json] {
            let notes = AnkiFile { path: path.clone() }.read_notes().await.unwrap();
            assert_eq!(notes.len(), 1, "{}", path.display());
            assert_eq!(notes[0].id, 10);
            let note = notes[0].to_note();
            assert_eq!(note.spares_id, Some("gir".parse().unwrap()));
            assert_eq!(note.note_data, "frontback");
            assert_eq!(note.tags, ["a", "b"]);
            let _ = std::fs::remove_file(path);
        }
    }
}
