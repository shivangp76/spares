use super::{BackReveal, FrontConceal};
use crate::helpers::parse_list;
use crate::model::{CustomData, NOTE_ID_KEY, NoteId};
use crate::parsers::Parseable;
use crate::{
    LibraryError, NoteErrorKind,
    adapters::{
        SrsAdapter,
        impls::spares::{SparesAdapter, SparesRequestProcessor},
    },
};
use serde_json::Value;
use std::ops::Range;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum NoteImportAction {
    #[default]
    Add,
    Update(NoteId),
    Delete(NoteId),
}

#[derive(Clone, Debug, Default)]
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
    pub custom_data: CustomData,
    /// Internal
    pub linked_notes: Vec<String>,
    /// Internal
    pub errors_and_warnings: Vec<LibraryError>,
    /// Internal
    pub cards_count: Option<usize>,
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
            ReadWriteValue::Different { read, .. } => read.iter().any(|&r| r == value),
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
    pub custom_data: ReadWriteValue,
    pub settings_delim: &'static str,
    pub settings_key_value_delim: &'static str,
    pub global_settings_prefix: ReadWriteValue,
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
            custom_data: ReadWriteValue::Same("custom-data"),
            settings_delim: ";",
            settings_key_value_delim: ":",
            global_settings_prefix: ReadWriteValue::Same("g-"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegexMatch {
    pub match_range: Range<usize>,
    pub capture_range: Range<usize>,
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

#[allow(clippy::too_many_lines)]
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
        custom_data: custom_data_key,
        global_settings_prefix: global_prefix,
        settings_delim,
        settings_key_value_delim,
    } = parser.note_settings_keys();
    let mut all_settings_split = Vec::new();
    for settings_indices in all_settings_indices {
        let settings_split = get_settings_pairs(
            data,
            settings_indices,
            settings_delim,
            settings_key_value_delim,
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
                    temp_action = Some(NoteImportAction::Add);
                } else if action_update_key.matches_read(value) {
                    temp_action = Some(NoteImportAction::Update(0));
                } else if action_delete_key.matches_read(value) {
                    temp_action = Some(NoteImportAction::Delete(0));
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
                if let Err(items) = parse_settings_list(value, &mut settings.tags, true) {
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
                if let Err(items) = parse_settings_list(value, &mut settings.keywords, false) {
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
            } else if custom_data_key.matches_read(key) {
                // TODO: Maybe convert this to TOML instead?
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
                match note_id_str.parse::<NoteId>() {
                    Ok(note_id_parsed) => {
                        if temp_note_id.is_none() {
                            temp_note_id = Some(note_id_parsed);
                        }
                    }
                    Err(_) => local_settings.errors_and_warnings.push(LibraryError::Note(
                        NoteErrorKind::InvalidSettings {
                            description: format!("The note id `{note_id_str_value}` is not valid."),
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
        (Some(NoteImportAction::Add) | None, None) => {}
        (Some(NoteImportAction::Add), Some(_)) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::SettingsWarning {
                    description: "Note id is not needed for adding a note.".to_string(),
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(NoteImportAction::Update(_)), None) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::InvalidSettings {
                    description: "Note id is needed for editing a note.".to_string(),
                    advice: None,
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(NoteImportAction::Delete(_)), None) => {
            local_settings.errors_and_warnings.push(LibraryError::Note(
                NoteErrorKind::InvalidSettings {
                    description: "Note id is needed for deleting a note.".to_string(),
                    advice: None,
                    src: data.to_string(),
                    at: note_capture.clone().into(),
                },
            ));
        }
        (Some(NoteImportAction::Update(_)) | None, Some(note_id)) => {
            local_settings.action = NoteImportAction::Update(note_id);
        }
        (Some(NoteImportAction::Delete(_)), Some(note_id)) => {
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
) -> Vec<Result<(&'a str, &'a str), (String, Range<usize>)>> {
    data[settings_indices.clone()]
        .split(settings_delim)
        .filter(|x| !x.is_empty())
        .map(|s| {
            s.splitn(2, settings_key_value_delim)
                .map(|x| x.trim())
                .collect::<Vec<_>>()
        })
        .map(|parts: Vec<&str>| {
            if parts.len() != 2 {
                dbg!(&parts);
                return Err((
                    format!(
                        "Found {} parts when processing settings. Expected 2 parts.",
                        parts.len(),
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
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let new_items = parse_list(value);
    for item in new_items {
        if let Some(stripped) = item.strip_prefix('-') {
            if stripped == "*" {
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
