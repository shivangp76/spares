use inquire::Select;
use reqwest::Client;

use crate::AdapterErrorKind;
use crate::Error;
use crate::LibraryError;
use crate::adapters::SrsAdapter;
use crate::adapters::impls::anki::ANKI_ADAPTER_NAME;
use crate::adapters::impls::anki::AnkiAdapter;
use crate::adapters::impls::anki::api::execute_requests;
use crate::adapters::impls::anki::types::ApiAction;
use crate::adapters::impls::anki::types::ApiRequest;
use crate::adapters::impls::anki::types::ApiRequestParams;
use crate::adapters::impls::anki::types::ModelName;
use crate::adapters::impls::anki::types::NoteFields;
use crate::adapters::impls::anki::types::UpdateNoteApiRequestData;
use crate::adapters::impls::anki::types::UpdateNoteApiRequestNoteData;
use crate::helpers::get_or_compile_regex;
use crate::parsers::NoteImportAction;
use crate::parsers::NotePart;
use crate::parsers::Parseable;
use crate::parsers::get_adapter_note_id_key;
use crate::parsers::image_occlusion::ConstructImageOcclusionType;
use crate::parsers::tts::TtsSetting;
use crate::parsers::tts::TtsVoice;
use crate::schema::note::NoteResponse;
use crate::schema::note::NotesResponse;

pub fn note_action_to_anki(note_action: NoteImportAction) -> ApiAction {
    match note_action {
        NoteImportAction::Add => ApiAction::AddNote,
        NoteImportAction::Update(_) => ApiAction::UpdateNote,
        NoteImportAction::Delete(_) => ApiAction::DeleteNote,
    }
}

// fn get_gui_browse_request(query: &str) -> ApiRequest {
//     ApiRequest {
//         action: ApiAction::GuiBrowse,
//         params: ApiRequestParams::GuiBrowse(GuiBrowseApiRequestData {
//             query: query.to_owned(),
//         }),
//         version: 6,
//     }
// }

impl AnkiAdapter {
    pub fn verify_anki_is_open(&mut self) -> Result<(), Error> {
        if self.confirm_bypass {
            return Ok(());
        }
        let abort_str = "Abort";
        let short_confirm_str = "Confirm";
        let long_confirm_str = "Confirm all";
        let options = vec![abort_str, short_confirm_str, long_confirm_str];
        let ans = Select::new("Please confirm that Anki is open.", options)
            .with_help_message("This is needed to import data into Anki.")
            .prompt();
        let abort = match ans {
            Ok(choice) => {
                if choice == abort_str {
                    true
                } else if choice == short_confirm_str {
                    false
                } else if choice == long_confirm_str {
                    self.confirm_bypass = true;
                    false
                } else {
                    unreachable!()
                }
            }
            Err(_) => true,
        };
        if abort {
            return Err(Error::Library(LibraryError::Adapter(
                AdapterErrorKind::Custom {
                    adapter_name: ANKI_ADAPTER_NAME.to_string(),
                    error: "Aborting since Anki is not open.".to_string(),
                },
            )));
        }
        Ok(())
    }

    pub async fn add_spares_id(
        &mut self,
        notes_responses: &[NotesResponse],
        client: &Client,
        dry_run: bool,
    ) -> Result<(), Error> {
        let mut requests = Vec::new();
        for note_response in notes_responses {
            for note in &note_response.notes {
                let anki_note_id = get_note_id(note).map_err(|e| {
                    Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
                        adapter_name: ANKI_ADAPTER_NAME.to_string(),
                        error: e.to_string(),
                    }))
                })?;
                let note_data = UpdateNoteApiRequestNoteData {
                    deck_name: "Default".to_string(),
                    model_name: ModelName::Basic,
                    id: anki_note_id,
                    fields: NoteFields {
                        front: None,
                        back: None,
                        keywords: None,
                        spares_id: Some(note.id.letters()),
                        spares_parser_name: None,
                    },
                    tags: None,
                };

                let api_request = ApiRequest {
                    action: ApiAction::UpdateNote,
                    params: ApiRequestParams::UpdateNote(UpdateNoteApiRequestData {
                        note: note_data,
                    }),
                    version: 6,
                };
                requests.push(api_request);
            }
        }
        if !dry_run && !requests.is_empty() {
            self.verify_anki_is_open()?;
        }
        execute_requests(&requests, dry_run, true, client).await?;
        Ok(())
    }

    pub fn note_parts_to_data(data: &[NotePart], parser: &dyn Parseable) -> String {
        data.iter()
            .map(|p| match p {
                NotePart::SurroundingData(text)
                | NotePart::ClozeData(text, _)
                | NotePart::ClozeStart(text)
                | NotePart::ClozeEnd(text) => text.clone(),
                NotePart::ImageOcclusion { data, .. } => {
                    parser.construct_image_occlusion(data, ConstructImageOcclusionType::Note)
                }
                NotePart::Cli { .. } => {
                    // Anki adapter does not support CLI cards (no rendered form);
                    // emit nothing.
                    String::new()
                }
            })
            .collect::<String>()
    }
}

pub fn to_anki_html(data: &str, is_latex: bool) -> String {
    let mut new_data = String::new();
    if is_latex {
        new_data.push_str("[latex]<br/>");
    }
    new_data.push_str(data);
    if is_latex {
        new_data.push_str("<br/>[/latex]");
    }
    new_data = new_data.replace('\n', "<br/>");
    new_data
}

pub fn format_side(data: &str) -> String {
    let mut data = data.to_string();

    // Latex prefix/suffix
    let latex_start = "[latex]\n";
    if data.starts_with(latex_start) {
        data = data[latex_start.len()..].to_string();
    }
    let latex_end = "\n[/latex]";
    if data.ends_with(latex_end) {
        data = data[..data.len() - latex_end.len()].to_string();
    }
    data
}

/// `data` with Anki's text to speech tags (`[anki:tts lang=fr_FR voices=Apple_Thomas
/// speed=0.8]bonjour[/anki:tts]`) replaced by their text, the settings of its tags, in order, and
/// whether text outside them was not spoken. Spares speaks text with the note setting `tts` instead
/// (see [`crate::parsers::tts`]).
pub fn take_anki_tts(data: &str) -> (String, Vec<TtsSetting>, bool) {
    let tag_regex =
        get_or_compile_regex(r"(?s)\[anki:tts(?:\s+([^\]]*))?\](.*?)\[/anki:tts\]").unwrap();
    let mut settings = Vec::new();
    let mut result = String::new();
    let mut unspoken = false;
    let mut start = 0;
    for caps in tag_regex.captures_iter(data).filter_map(Result::ok) {
        let tag = caps.get(0).unwrap();
        let mut voice = TtsVoice::default();
        for attribute in caps.get(1).map_or("", |m| m.as_str()).split_whitespace() {
            match attribute.split_once('=') {
                Some(("lang", lang)) => voice.lang = Some(lang.to_string()),
                Some(("speed", speed)) => voice.speed = speed.parse().ok(),
                Some(("voices", voices)) => {
                    // The first voice, without the platform Anki prefixes it with
                    voice.voice = voices.split(',').next().map(|name| {
                        ["Apple_", "Microsoft_", "Google_"]
                            .iter()
                            .find_map(|prefix| name.strip_prefix(prefix))
                            .unwrap_or(name)
                            .to_string()
                    });
                }
                _ => {}
            }
        }
        settings.push(TtsSetting::Speak(voice));
        unspoken |= !data[start..tag.start()].trim().is_empty();
        result.push_str(&data[start..tag.start()]);
        result.push_str(&caps[2]);
        start = tag.end();
    }
    unspoken |= !data[start..].trim().is_empty();
    result.push_str(&data[start..]);
    (result, settings, unspoken)
}

pub fn get_note_id(note_response: &NoteResponse) -> Result<i64, Error> {
    let anki_note_id_str = note_response
        .custom_data
        .iter()
        .find(|(k, _v)| **k == get_adapter_note_id_key(AnkiAdapter::new().get_adapter_name()))
        .ok_or(Error::Library(LibraryError::Adapter(
            AdapterErrorKind::Custom {
                adapter_name: ANKI_ADAPTER_NAME.to_string(),
                error: "Failed to get anki note id custom field".to_string(),
            },
        )))?
        .1
        .as_str()
        .unwrap();
    anki_note_id_str.trim().parse::<i64>().map_err(|e| {
        Error::Library(LibraryError::Adapter(AdapterErrorKind::Custom {
            adapter_name: ANKI_ADAPTER_NAME.to_string(),
            error: e.to_string(),
        }))
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_take_anki_tts() {
        let (text, settings, unspoken) = take_anki_tts(
            "Hello: [anki:tts lang=fr_FR voices=Apple_Thomas,Microsoft_Paul speed=0.8]bonjour\
             [/anki:tts] [anki:tts]hi[/anki:tts]",
        );
        assert_eq!(text, "Hello: bonjour hi");
        assert_eq!(
            settings,
            [
                TtsSetting::Speak(TtsVoice {
                    lang: Some("fr_FR".to_string()),
                    speed: Some(0.8),
                    voice: Some("Thomas".to_string()),
                }),
                TtsSetting::Speak(TtsVoice::default()),
            ]
        );
        assert!(unspoken);

        let (text, settings, unspoken) = take_anki_tts(" [anki:tts lang=de]Hund[/anki:tts]\n");
        assert_eq!(text, " Hund\n");
        assert_eq!(settings.len(), 1);
        assert!(!unspoken);

        assert_eq!(
            take_anki_tts("plain"),
            ("plain".to_string(), Vec::new(), true)
        );
    }
}
