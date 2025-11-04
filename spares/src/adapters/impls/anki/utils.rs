use crate::adapters::SrsAdapter;
use crate::adapters::impls::anki::api::execute_requests;
use crate::adapters::impls::anki::types::{
    ApiAction, ApiRequest, ApiRequestParams, ModelName, NoteFields, UpdateNoteApiRequestData,
    UpdateNoteApiRequestNoteData,
};
use crate::adapters::impls::anki::{ANKI_ADAPTER_NAME, AnkiAdapter};
use crate::parsers::{
    NoteImportAction, NotePart, Parseable, get_adapter_note_id_key,
    image_occlusion::ConstructImageOcclusionType,
};
use crate::schema::note::{NoteResponse, NotesResponse};
use crate::{AdapterErrorKind, Error, LibraryError};
use inquire::Select;
use reqwest::Client;

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
        run: bool,
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
                        spares_id: Some(note.id.to_string()),
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
        if run && !requests.is_empty() {
            self.verify_anki_is_open()?;
        }
        execute_requests(&requests, run, true, client).await?;
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
