//! What rendering this machine's files recorded, for each of a note's outputs (its own file and
//! each side of its cards), in the `render_output` table: the [render key](RenderState::key) that
//! tells whether a rendered file is up to date, and the files it read ([`RenderDependencies`]),
//! which [`super::browser_render::sync_render_assets`] stores for web clients and `spares sync
//! cloud` copies to the server.
//!
//! They are in the database rather than in files next to the rendered ones, so a note's rows go
//! with it, nothing has to scan the rendered files' directories, and the states of a batch of
//! notes are read in one query before rendering it ([`read_render_states`]), and written in one
//! transaction after ([`write_render_states`]), since notes render on threads without the
//! database.
//!
//! The table is local: each machine renders its own files, so it isn't synced, and a full sync
//! doesn't copy it.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::path::PathBuf;

use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::model::NoteId;
use crate::parsers::RenderDependencies;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::NoteFiles;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::generate_files::RenderState;
use crate::parsers::generate_files::RenderStates;

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// `output` as the `render_output` table names it
fn output_name(output: RenderOutputType) -> String {
    match output {
        RenderOutputType::Note => String::new(),
        RenderOutputType::Card(order, CardSide::Front) => format!("{order}-front"),
        RenderOutputType::Card(order, CardSide::Back) => format!("{order}-back"),
    }
}

fn parse_output_name(name: &str) -> Option<RenderOutputType> {
    if name.is_empty() {
        return Some(RenderOutputType::Note);
    }
    let (order, side) = name.split_once('-')?;
    let side = match side {
        "front" => CardSide::Front,
        "back" => CardSide::Back,
        _ => return None,
    };
    Some(RenderOutputType::Card(order.parse().ok()?, side))
}

/// `ids` as a JSON array, for `json_each`
fn ids_json<'a>(ids: impl IntoIterator<Item = &'a NoteId>) -> String {
    serde_json::to_string(&ids.into_iter().collect::<Vec<_>>()).unwrap_or_default()
}

/// The [`RenderStates`] of the notes `note_ids`, of those that have any.
pub async fn read_render_states(
    db: &SqlitePool,
    note_ids: &[NoteId],
) -> Result<HashMap<NoteId, RenderStates>, Error> {
    let rows: Vec<(NoteId, String, String, String)> = sqlx::query_as(
        "SELECT note_id, output, key, dependencies FROM render_output
         WHERE note_id IN (SELECT value FROM json_each(?))",
    )
    .bind(ids_json(note_ids))
    .fetch_all(db)
    .await
    .map_err(sqlx_error)?;
    let mut states: HashMap<NoteId, RenderStates> = HashMap::new();
    for (note_id, output, key, dependencies) in rows {
        let Some(output) = parse_output_name(&output) else {
            continue;
        };
        // Unreadable dependencies are recorded again when the file is next rendered
        let dependencies = serde_json::from_str(&dependencies).unwrap_or_default();
        states
            .entry(note_id)
            .or_default()
            .insert(output, RenderState { key, dependencies });
    }
    Ok(states)
}

/// Stores what rendering `files` changed in their notes' [`RenderStates`]. A note deleted since is
/// skipped.
pub async fn write_render_states(db: &SqlitePool, files: &[NoteFiles]) -> Result<(), Error> {
    if files
        .iter()
        .all(|f| f.render_states.rendered.is_empty() && f.render_states.removed.is_empty())
    {
        return Ok(());
    }
    let mut tx = db.begin().await.map_err(sqlx_error)?;
    for files in files {
        for output in &files.render_states.removed {
            sqlx::query("DELETE FROM render_output WHERE note_id = ? AND output = ?")
                .bind(files.note_id)
                .bind(output_name(*output))
                .execute(&mut *tx)
                .await
                .map_err(sqlx_error)?;
        }
        for (output, state) in &files.render_states.rendered {
            insert_state(&mut tx, files.note_id, *output, state).await?;
        }
    }
    tx.commit().await.map_err(sqlx_error)
}

async fn insert_state(
    tx: &mut sqlx::SqliteConnection,
    note_id: NoteId,
    output: RenderOutputType,
    state: &RenderState,
) -> Result<(), Error> {
    let dependencies = serde_json::to_string(&state.dependencies).map_err(|e| Error::Io {
        description: "Failed to serialize render dependencies".to_string(),
        source: std::io::Error::other(e),
    })?;
    sqlx::query(
        "INSERT INTO render_output (note_id, output, key, dependencies)
         SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM note WHERE id = ?1)
         ON CONFLICT (note_id, output) DO UPDATE
         SET key = excluded.key, dependencies = excluded.dependencies",
    )
    .bind(note_id)
    .bind(output_name(output))
    .bind(&state.key)
    .bind(dependencies)
    .execute(&mut *tx)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

/// The files and Typst packages that rendering read for the notes of the parser `parser_name` in
/// `note_ids` (all of its notes if `None`).
pub async fn read_render_dependencies<S: BuildHasher>(
    db: &SqlitePool,
    parser_name: &str,
    note_ids: Option<&HashSet<NoteId, S>>,
) -> Result<(BTreeSet<PathBuf>, BTreeSet<String>), Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        r"SELECT r.dependencies FROM render_output r
        JOIN note n ON n.id = r.note_id JOIN parser p ON p.id = n.parser_id
        WHERE p.name = ?1 AND (?2 IS NULL OR r.note_id IN (SELECT value FROM json_each(?2)))",
    )
    .bind(parser_name)
    .bind(note_ids.map(ids_json))
    .fetch_all(db)
    .await
    .map_err(sqlx_error)?;
    let mut files = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for row in rows {
        if let Ok(dependencies) = serde_json::from_str::<RenderDependencies>(&row) {
            files.extend(dependencies.files);
            packages.extend(dependencies.packages);
        }
    }
    Ok((files, packages))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use chrono::Utc;
    use sqlx::SqlitePool;

    use super::parse_output_name;
    use super::read_render_states;
    use crate::api::note::create_notes;
    use crate::api::note::render_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::model::NoteId;
    use crate::parsers::Parseable;
    use crate::parsers::RenderOutputDirectoryType;
    use crate::parsers::generate_files::CardSide;
    use crate::parsers::generate_files::RenderOutputType;
    use crate::parsers::get_all_parsers;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::RenderNotesRequest;

    async fn render(pool: &SqlitePool, force: bool) {
        let request = RenderNotesRequest {
            selector: NotesSelector::All,
            immutable_note_ids: None,
            overridden_output_raw_dir: None,
            include_linked_notes: false,
            include_cards: true,
            generate_rendered: true,
            force_generate_rendered: force,
        };
        render_notes(pool, request, &get_all_parsers())
            .await
            .unwrap();
    }

    /// Rendering records each output's key, and renders again only what changed: outputs whose
    /// rendered file is gone, all of them when forced, and the note and its cards when it changed,
    /// forgetting the cards it no longer has.
    #[sqlx::test]
    async fn rendering_keeps_what_it_rendered_in_the_database(pool: SqlitePool) {
        let parser_row = create_parser_helper(&pool, "markdown").await;
        // Rendered files are in a directory every test shares, so the note has an id of its own
        sqlx::query("INSERT INTO id_counter (replica, tbl, last) VALUES (0, 'note', ?)")
            .bind(NoteId::from_letters("rsta").unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let request = CreateNotesRequest {
            parser_id: parser_row.id,
            requests: vec![CreateNoteRequest {
                data: "{{Paris}} is in France and {{Berlin}} in Germany.".to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: serde_json::Map::new(),
            }],
        };
        create_notes(&pool, request, Utc::now(), &get_all_parsers(), false)
            .await
            .unwrap();
        let note_id: NoteId = sqlx::query_scalar("SELECT id FROM note")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(note_id > NoteId::from_letters("rsta").unwrap());
        let parser = MarkdownParser::new();
        let rendered = |output| -> PathBuf {
            let dir = match output {
                RenderOutputType::Note => RenderOutputDirectoryType::Note,
                RenderOutputType::Card(..) => RenderOutputDirectoryType::Card,
            };
            parser
                .get_output_rendered_dir(dir)
                .join(parser.get_output_filename(output, note_id))
        };
        let modified = |output| -> SystemTime {
            std::fs::metadata(rendered(output))
                .unwrap()
                .modified()
                .unwrap()
        };
        let states = async || read_render_states(&pool, &[note_id]).await.unwrap();
        let note = RenderOutputType::Note;
        let card_1 = RenderOutputType::Card(1, CardSide::Front);
        let card_2 = RenderOutputType::Card(2, CardSide::Front);

        render(&pool, false).await;
        let first = states().await.remove(&note_id).unwrap();
        let mut outputs: Vec<_> = first.keys().copied().collect();
        outputs.sort_by_key(|output| super::output_name(*output));
        assert_eq!(outputs, [note, card_1, card_2]);
        let times: HashMap<_, _> = outputs.iter().map(|&o| (o, modified(o))).collect();

        // Up to date
        render(&pool, false).await;
        for &output in &outputs {
            assert_eq!(modified(output), times[&output], "{output:?}");
        }
        assert_eq!(states().await[&note_id], first);

        // Rendered files that are gone. Only the note's is looked at before its cards are made, as
        // a card is only rendered again with its note.
        for output in [note, card_2] {
            std::fs::remove_file(rendered(output)).unwrap();
        }
        render(&pool, false).await;
        assert!(rendered(note).exists());
        assert!(rendered(card_2).exists());
        assert_eq!(modified(card_1), times[&card_1]);

        // Forced
        render(&pool, true).await;
        assert_ne!(modified(card_1), times[&card_1]);

        // A note with one card fewer
        sqlx::query("UPDATE note SET data = '{{Paris}} is in France.'")
            .execute(&pool)
            .await
            .unwrap();
        render(&pool, false).await;
        let changed = states().await.remove(&note_id).unwrap();
        assert!(!changed.contains_key(&card_2));
        assert_ne!(changed[&note].key, first[&note].key);

        // A deleted note's rows go with it
        sqlx::query("DELETE FROM note")
            .execute(&pool)
            .await
            .unwrap();
        assert!(states().await.is_empty());
    }

    #[test]
    fn output_names_round_trip() {
        for output in [
            RenderOutputType::Note,
            RenderOutputType::Card(1, CardSide::Front),
            RenderOutputType::Card(12, CardSide::Back),
        ] {
            assert_eq!(parse_output_name(&super::output_name(output)), Some(output));
        }
        assert_eq!(parse_output_name("1-side"), None);
        assert_eq!(parse_output_name("x-front"), None);
    }
}
