use std::ops::Range;
use std::str::FromStr;

use serde_json::Map;
use serde_json::Value;

use super::BackReveal;
use super::FrontConceal;
use super::TypeAnswer;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::adapters::SrsAdapter;
use crate::adapters::impls::spares::SparesAdapter;
use crate::adapters::impls::spares::SparesRequestProcessor;
use crate::helpers::parse_list;
use crate::model::CustomData;
use crate::model::NOTE_ID_KEY;
use crate::model::NoteId;
use crate::parsers::DEFAULT_BACK_EMPHASIS;
use crate::parsers::Parseable;
use crate::parsers::tts::TtsSetting;

/// The action named in a note's settings, before its note id is known.
#[derive(Clone, Copy)]
enum ActionKind {
    Add,
    Update,
    Delete,
}

/// What an imported note asks for. Its id is spares' note id, except for adapters other than
/// spares, whose own note id (e.g. Anki's) is carried unchanged as the packed value (see
/// [`NoteId::from_packed`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum NoteImportAction {
    #[default]
    Add,
    Update(NoteId),
    Delete(NoteId),
}

#[derive(Clone, Debug)]
pub struct NoteSettings {
    pub action: NoteImportAction,
    pub tags: Vec<String>,
    pub keywords: Vec<String>,
    /// Shortcut for suspending all of a note's cards on creation
    pub is_suspended: bool,
    /// Shortcut for applying this settings to all of the note's cards
    pub front_conceal: FrontConceal,
    /// Shortcut for applying this settings to all of the note's cards
    pub back_reveal: BackReveal,
    /// Shortcut for applying this settings to all of the note's cards
    pub back_emphasis: bool,
    /// Shortcut for applying this settings to all of the note's cards
    pub type_answer: Option<TypeAnswer>,
    pub custom_data: CustomData,
    /// Internal
    pub linked_notes: Vec<String>,
    /// Internal
    pub errors_and_warnings: Vec<LibraryError>,
    /// Internal
    pub cards_count: Option<usize>,
}

impl Default for NoteSettings {
    fn default() -> Self {
        Self {
            action: NoteImportAction::default(),
            tags: Vec::default(),
            keywords: Vec::default(),
            is_suspended: Default::default(),
            front_conceal: FrontConceal::default(),
            back_reveal: BackReveal::default(),
            back_emphasis: DEFAULT_BACK_EMPHASIS,
            type_answer: None,
            custom_data: Map::default(),
            linked_notes: Vec::default(),
            errors_and_warnings: Vec::default(),
            cards_count: Option::default(),
        }
    }
}

// Each value can either be read and written with the same string,
// or have different strings for reading vs writing
#[derive(Clone, Debug)]
pub enum ReadWriteValue {
    Same(&'static str),
    Different {
        read: Vec<&'static str>,
        write: &'static str,
    },
}

impl ReadWriteValue {
    pub fn get_write(&self) -> &'static str {
        match self {
            ReadWriteValue::Same(s) => s,
            ReadWriteValue::Different { write, .. } => write,
        }
    }

    pub fn matches_read(&self, value: &str) -> bool {
        match self {
            ReadWriteValue::Same(s) => *s == value,
            ReadWriteValue::Different { read, .. } => read.contains(&value),
        }
    }
}

// Ideally would like to use serde here (see <https://stackoverflow.com/questions/71031746/how-to-get-renamed-enum-name-from-enum-value>), but then this wouldn't be customizable for each parser.
#[derive(Clone, Debug)]
pub struct NoteSettingsKeys {
    pub note_id: ReadWriteValue,
    pub action: ReadWriteValue,
    pub action_add: ReadWriteValue,
    pub action_update: ReadWriteValue,
    pub action_delete: ReadWriteValue,
    pub tags: ReadWriteValue,
    pub keywords: ReadWriteValue,
    pub is_suspended: ReadWriteValue,
    pub front_conceal: ReadWriteValue,
    pub back_reveal: ReadWriteValue,
    pub back_emphasis: ReadWriteValue,
    pub type_answer: ReadWriteValue,
    pub custom_data: ReadWriteValue,
    /// A global setting that makes the notes after it live notes of the named sync group: they are
    /// matched to the database by their position in the file, so a file can be edited and imported
    /// again. See "Live notes" in the docs of `spares_cli::import`.
    pub live_sync_name: ReadWriteValue,
    /// Speaks the note's text. It is stored in the note's custom data, under
    /// [`TtsSetting::KEY`]. See [`crate::parsers::tts`].
    pub tts: ReadWriteValue,
    pub settings_delim: &'static str,
    pub settings_key_value_delim: &'static str,
    pub global_settings_prefix: ReadWriteValue,
    pub groupings_all: &'static str,
}

impl Default for NoteSettingsKeys {
    fn default() -> Self {
        Self {
            // NOTE: snake_case won't work in latex due to the underscore
            note_id: ReadWriteValue::Same("note-id"),
            action: ReadWriteValue::Same("action"),
            action_add: ReadWriteValue::Same("add"),
            action_update: ReadWriteValue::Same("update"),
            action_delete: ReadWriteValue::Same("delete"),
            tags: ReadWriteValue::Different {
                read: vec!["tags", "t"],
                write: "tags",
            },
            keywords: ReadWriteValue::Different {
                read: vec!["keywords", "k"],
                write: "keywords",
            },
            is_suspended: ReadWriteValue::Same("is-suspended"),
            front_conceal: ReadWriteValue::Same("front-conceal"),
            back_reveal: ReadWriteValue::Same("back-reveal"),
            back_emphasis: ReadWriteValue::Same("back-emphasis"),
            type_answer: ReadWriteValue::Same("type-answer"),
            custom_data: ReadWriteValue::Same("custom-data"),
            live_sync_name: ReadWriteValue::Same("live-sync-name"),
            tts: ReadWriteValue::Same(TtsSetting::KEY),
            settings_delim: ";",
            settings_key_value_delim: ":",
            global_settings_prefix: ReadWriteValue::Same("g-"),
            groupings_all: "*",
        }
    }
}

pub fn get_adapter_note_id_key(adapter_name: &str) -> String {
    let default_adapter =
        adapter_name == SparesAdapter::new(SparesRequestProcessor::Server).get_adapter_name();
    if default_adapter {
        NOTE_ID_KEY.to_string()
    } else {
        format!("{}-{}", adapter_name, NOTE_ID_KEY)
    }
}

#[expect(clippy::too_many_lines)]
pub fn parse_note_settings(
    parser: &dyn Parseable,
    data: &str,
    all_settings_indices: &[Range<usize>],
    global_settings: &mut NoteSettings,
    local_settings: &mut NoteSettings,
    adapter: &dyn SrsAdapter,
    note_capture: &Range<usize>,
) {
    let NoteSettingsKeys {
        note_id: note_id_key,
        action: action_key,
        action_add: action_add_key,
        action_update: action_update_key,
        action_delete: action_delete_key,
        tags: tags_keys,
        keywords: keywords_keys,
        is_suspended: is_suspended_key,
        front_conceal: front_conceal_key,
        back_reveal: back_reveal_key,
        back_emphasis: back_emphasis_key,
        type_answer: type_answer_key,
        custom_data: custom_data_key,
        live_sync_name: live_sync_name_key,
        tts: tts_key,
        global_settings_prefix: global_prefix,
        settings_delim,
        settings_key_value_delim,
        groupings_all,
    } = parser.note_settings_keys();
    let mut all_settings_split = Vec::new();
    for settings_indices in all_settings_indices {
        let settings_split = get_settings_pairs(
            data,
            settings_indices,
            settings_delim,
            settings_key_value_delim,
            false,
        )
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|(description, indices)| {
            LibraryError::Note(NoteErrorKind::InvalidSettings {
                description,
                advice: None,
                src: data.to_string(),
                at: indices.into(),
            })
        });
        if let Err(settings_err) = settings_split {
            local_settings.errors_and_warnings.push(settings_err);
            return;
        }
        all_settings_split.extend(
            settings_split
                .unwrap()
                .into_iter()
                .map(|(k, v)| (settings_indices, k, v)),
        );
    }
    let adapter_name = adapter.get_adapter_name();
    let default_adapter =
        adapter_name == SparesAdapter::new(SparesRequestProcessor::Server).get_adapter_name();
    let adapter_note_id_key = if default_adapter {
        note_id_key.get_write().to_string()
    } else {
        format!("{}-{}", adapter_name, note_id_key.get_write())
    };
    let mut temp_action = None;
    let mut temp_note_id: Option<NoteId> = None;
    for (settings_indices, mut key, value) in all_settings_split {
        let mut settings_to_change: Vec<&mut NoteSettings> = vec![local_settings];
        if key.starts_with(global_prefix.get_write()) {
            key = &key[global_prefix.get_write().len()..];
            settings_to_change.push(global_settings);
        }
        for settings in settings_to_change {
            if action_key.matches_read(key) {
                if action_add_key.matches_read(value) {
                    temp_action = Some(ActionKind::Add);
                } else if action_update_key.matches_read(value) {
                    temp_action = Some(ActionKind::Update);
                } else if action_delete_key.matches_read(value) {
                    temp_action = Some(ActionKind::Delete);
                } else {
                    settings.errors_and_warnings.push(LibraryError::Note(
                        NoteErrorKind::InvalidSettings {
                            description: format!("The action `{value}` is not supported."),
                            advice: None,
                            src: data.to_string(),
                            at: settings_indices.clone().into(),
                        },
                    ));
                }
            } else if tags_keys.matches_read(key) {
                if let Err(items) =
                    parse_settings_list(value, &mut settings.tags, true, groupings_all)
                {
                    settings.errors_and_warnings.extend(
                        items
                            .into_iter()
                            .map(|item| LibraryError::Note(NoteErrorKind::SettingsWarning {
                                description: format!("The item `{item}` was not found, so it could not be removed."),
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            }))
                            .collect::<Vec<_>>(),
                    );
                }
            } else if keywords_keys.matches_read(key) {
                if let Err(items) =
                    parse_settings_list(value, &mut settings.keywords, false, groupings_all)
                {
                    settings.errors_and_warnings.extend(
                        items
                            .into_iter()
                            .map(|item| LibraryError::Note(NoteErrorKind::SettingsWarning {
                                description: format!("The item `{item}` was not found, so it could not be removed."),
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            }))
                            .collect::<Vec<_>>(),
                    );
                }
                if settings.keywords.iter().any(|x| x.contains("TODO")) {
                    settings.errors_and_warnings.push(LibraryError::Note(
                        NoteErrorKind::SettingsWarning {
                            description: "The field `keywords` contains TODO.".to_string(),
                            src: data.to_string(),
                            at: settings_indices.clone().into(),
                        },
                    ));
                }
            } else if is_suspended_key.matches_read(key) {
                settings.is_suspended = true;
            } else if front_conceal_key.matches_read(key) {
                let front_conceal_res = FrontConceal::from_str(value);
                match front_conceal_res {
                    Ok(front_conceal) => settings.front_conceal = front_conceal,
                    Err(e) => {
                        settings.errors_and_warnings.push(LibraryError::Note(
                            NoteErrorKind::InvalidSettings {
                                description: format!("Failed to parse front conceal: {}", e),
                                advice: None,
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            },
                        ));
                    }
                }
            } else if back_reveal_key.matches_read(key) {
                let back_reveal_res = BackReveal::from_str(value);
                match back_reveal_res {
                    Ok(back_reveal) => settings.back_reveal = back_reveal,
                    Err(e) => {
                        settings.errors_and_warnings.push(LibraryError::Note(
                            NoteErrorKind::InvalidSettings {
                                description: format!("Failed to parse front conceal: {}", e),
                                advice: None,
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            },
                        ));
                    }
                }
            } else if back_emphasis_key.matches_read(key) {
                settings.back_emphasis = true;
            } else if type_answer_key.matches_read(key) {
                match TypeAnswer::from_str(value) {
                    Ok(type_answer) => settings.type_answer = Some(type_answer),
                    Err(e) => {
                        settings.errors_and_warnings.push(LibraryError::Note(
                            NoteErrorKind::InvalidSettings {
                                description: format!("Failed to parse type answer: {}", e),
                                advice: None,
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            },
                        ));
                    }
                }
            } else if custom_data_key.matches_read(key) {
                let parsed_custom_data: Result<CustomData, _> = serde_json::from_str(value);
                match parsed_custom_data {
                    Ok(custom_data) => {
                        settings.custom_data.extend(custom_data);
                    }
                    Err(e) => {
                        settings.errors_and_warnings.push(LibraryError::Note(
                            NoteErrorKind::InvalidSettings {
                                description: format!("Failed to parse custom data: {}", e),
                                advice: None,
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            },
                        ));
                    }
                }
            } else if tts_key.matches_read(key) {
                match TtsSetting::from_str(value) {
                    Ok(tts) => {
                        settings
                            .custom_data
                            .insert(TtsSetting::KEY.to_string(), Value::String(tts.to_string()));
                    }
                    Err(e) => {
                        settings.errors_and_warnings.push(LibraryError::Note(
                            NoteErrorKind::InvalidSettings {
                                description: format!("Failed to parse text to speech: {}", e),
                                advice: None,
                                src: data.to_string(),
                                at: settings_indices.clone().into(),
                            },
                        ));
                    }
                }
            } else if live_sync_name_key.matches_read(key) {
                settings.custom_data.insert(
                    "live_sync_name".to_string(),
                    Value::String(value.to_string()),
                );
            } else {
                settings
                    .custom_data
                    .insert(key.to_string(), Value::String(value.to_string()));
                // let adding_to_custom_data_str = if default_adapter {
                //     "Added to custom_data."
                // } else {
                //     ""
                // };
                // settings.warnings.push(format!(
                //     "The key {} is not supported. {}",
                //     key, adding_to_custom_data_str
                // ));
            }
        }
        if let Some(note_id_str_value) = local_settings
            .custom_data
            .shift_remove(&adapter_note_id_key)
        {
            let note_id_str_res: Result<String, _> =
                serde_json::from_value(note_id_str_value.clone()).map_err(|_| {
                    local_settings.errors_and_warnings.push(LibraryError::Note(
                        NoteErrorKind::InvalidSettings {
                            description: format!("The note id `{note_id_str_value}` is not valid."),
                            advice: None,
                            src: data.to_string(),
                            at: settings_indices.clone().into(),
                        },
                    ));
                });
            if let Ok(note_id_str) = note_id_str_res {
                let parsed = if default_adapter {
                    note_id_str.parse::<NoteId>().map_err(|e| e.to_string())
                } else {
                    note_id_str
                        .trim()
                        .parse::<i64>()
                        .map(NoteId::from_packed)
                        .map_err(|e| e.to_string())
                };
                match parsed {
                    Ok(note_id_parsed) => {
                        if temp_note_id.is_none() {
                            temp_note_id = Some(note_id_parsed);
                        }
                    }
                    Err(e) => local_settings.errors_and_warnings.push(LibraryError::Note(
                        NoteErrorKind::InvalidSettings {
                            description: format!(
                                "The note id `{note_id_str_value}` is not valid: {e}"
                            ),
                            advice: None,
                            src: data.to_string(),
                            at: settings_indices.clone().into(),
                        },
                    )),
                }
            }
        }
        // Replace note id keys
        if note_id_key.get_write() != NOTE_ID_KEY {
            if let Some(value) = local_settings.custom_data.get(note_id_key.get_write()) {
                local_settings
                    .custom_data
                    .insert(NOTE_ID_KEY.to_string(), value.clone());
                // `.swap_remove()` replaces it with the last element which is the key we just inserted. Thus, this is equivalent to replacing the key, while preserving order.
                local_settings
                    .custom_data
                    .swap_remove(note_id_key.get_write());
            }
            if let Some(value) = local_settings.custom_data.get(&adapter_note_id_key) {
                let new_key = format!("{}-{}", adapter_name, NOTE_ID_KEY);
                local_settings.custom_data.insert(new_key, value.clone());
                // `.swap_remove()` replaces it with the last element which is the key we just inserted. Thus, this is equivalent to replacing the key, while preserving order.
                local_settings.custom_data.swap_remove(&adapter_note_id_key);
            }
        }
    }
    // Validate settings
    match (temp_action, temp_note_id) {
        (Some(ActionKind::Add) | None, None) => {}
        (Some(ActionKind::Add), Some(_)) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::SettingsWarning {
                    description: "Note id is not needed for adding a note.".to_string(),
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(ActionKind::Update), None) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::InvalidSettings {
                    description: "Note id is needed for editing a note.".to_string(),
                    advice: None,
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(ActionKind::Delete), None) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::InvalidSettings {
                    description: "Note id is needed for deleting a note.".to_string(),
                    advice: None,
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(ActionKind::Update) | None, Some(note_id)) => {
            local_settings.action = NoteImportAction::Update(note_id);
        }
        (Some(ActionKind::Delete), Some(note_id)) => {
            local_settings.action = NoteImportAction::Delete(note_id);
        }
    }
}

#[allow(
    clippy::type_complexity,
    reason = "only the error variant is too complex"
)]
pub fn get_settings_pairs<'a>(
    data: &'a str,
    settings_indices: &Range<usize>,
    settings_delim: &str,
    settings_key_value_delim: &str,
    // Whether a key without a value, such as `t` in `{{[t] Paris}}`, is read as having an empty
    // value, like `t:`.
    allow_bare_keys: bool,
) -> Vec<Result<(&'a str, &'a str), (String, Range<usize>)>> {
    data[settings_indices.clone()]
        .split(settings_delim)
        .filter(|x| !x.is_empty())
        .map(|s| {
            s.splitn(2, settings_key_value_delim)
                .map(|x| x.trim())
                .collect::<Vec<_>>()
        })
        .map(|mut parts: Vec<&str>| {
            if allow_bare_keys && parts.len() == 1 && !parts[0].is_empty() {
                parts.push("");
            }
            if parts.len() != 2 {
                return Err((
                    format!(
                        "Found {} parts in `{:?}` when processing settings. Expected 2 parts.",
                        parts.len(),
                        parts,
                    ),
                    settings_indices.clone(),
                ));
            }
            let mut parts_iter = parts.into_iter();
            Ok((parts_iter.next().unwrap(), parts_iter.next().unwrap()))
        })
        .collect::<Vec<_>>()
}

/// The error value is a list of all items that could not be found.
fn parse_settings_list(
    value: &str,
    existing: &mut Vec<String>,
    sort: bool,
    groupings_all: &str,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let new_items = parse_list(value);
    for item in new_items {
        if let Some(stripped) = item.strip_prefix('-') {
            if stripped == groupings_all {
                existing.clear();
            } else {
                let index = existing.iter().position(|x| *x == stripped);
                match index {
                    Some(i) => {
                        existing.remove(i);
                    }
                    None => {
                        errors.push(stripped.to_string());
                    }
                }
            }
        } else if !item.is_empty() {
            existing.push(item);
        }
    }
    if sort {
        existing.sort();
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_sync_name_stored_in_custom_data() {
        let parser: Box<dyn crate::parsers::Parseable> =
            Box::new(crate::parsers::impls::markdown::MarkdownParser::new());
        let adapter = crate::adapters::impls::spares::SparesAdapter::new(
            crate::adapters::impls::spares::SparesRequestProcessor::Server,
        );

        // A minimal note with a global live-sync-name setting
        let data = "<!--- # g-live-sync-name: lecture_notes-501 --->\n\
                     <!--- # note-id: 1 --->\n\
                     <!--- spares: note start --->\n\
                     some content\n\
                     <!--- spares: note end --->\n";

        let settings_ranges = parser.as_ref().get_settings(data).unwrap();
        let mut global_settings = NoteSettings::default();
        let mut local_settings = NoteSettings::default();
        let note_range = 0..data.len();

        parse_note_settings(
            parser.as_ref(),
            data,
            &settings_ranges,
            &mut global_settings,
            &mut local_settings,
            &adapter,
            &note_range,
        );

        assert_eq!(
            local_settings
                .custom_data
                .get("live_sync_name")
                .and_then(|v| v.as_str()),
            Some("lecture_notes-501"),
        );
        // Also check global_settings has it
        assert_eq!(
            global_settings
                .custom_data
                .get("live_sync_name")
                .and_then(|v| v.as_str()),
            Some("lecture_notes-501"),
        );
    }

    #[test]
    fn test_tts_stored_in_custom_data() {
        let parser = crate::parsers::impls::markdown::MarkdownParser::new();
        let adapter = crate::adapters::impls::spares::SparesAdapter::new(
            crate::adapters::impls::spares::SparesRequestProcessor::Server,
        );
        let parse = |tts: &str| {
            let data = format!(
                "<!--- # tts: {tts} --->\n<!--- spares: note start --->\nx\n<!--- spares: note end --->\n"
            );
            let settings_ranges = parser.get_settings(&data).unwrap();
            let mut global_settings = NoteSettings::default();
            let mut local_settings = NoteSettings::default();
            parse_note_settings(
                &parser,
                &data,
                &settings_ranges,
                &mut global_settings,
                &mut local_settings,
                &adapter,
                &(0..data.len()),
            );
            local_settings
        };

        let settings = parse("fr_FR, 0.80");
        assert_eq!(
            settings.custom_data.get(TtsSetting::KEY),
            Some(&Value::String("fr_FR,0.8".to_string()))
        );
        assert!(settings.errors_and_warnings.is_empty());

        let settings = parse("fr_FR,fast");
        assert!(settings.custom_data.get(TtsSetting::KEY).is_none());
        assert_eq!(settings.errors_and_warnings.len(), 1);
    }

    #[test]
    fn test_non_live_import_ignores_key() {
        let parser: Box<dyn crate::parsers::Parseable> =
            Box::new(crate::parsers::impls::markdown::MarkdownParser::new());
        let adapter = crate::adapters::impls::spares::SparesAdapter::new(
            crate::adapters::impls::spares::SparesRequestProcessor::Server,
        );

        // No g-live-sync-name in settings
        let data = "<!--- # note-id: 1 --->\n\
                     <!--- spares: note start --->\n\
                     some content\n\
                     <!--- spares: note end --->\n";

        let settings_ranges = parser.as_ref().get_settings(data).unwrap();
        let mut global_settings = NoteSettings::default();
        let mut local_settings = NoteSettings::default();
        let note_range = 0..data.len();

        parse_note_settings(
            parser.as_ref(),
            data,
            &settings_ranges,
            &mut global_settings,
            &mut local_settings,
            &adapter,
            &note_range,
        );

        assert!(local_settings.custom_data.get("live_sync_name").is_none());
    }
}
