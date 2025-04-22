use super::image_occlusion::get_image_occlusion_rendered_directory;
use crate::Error;
use crate::model::NoteId;
use crate::parsers::image_occlusion::create_image_occlusion_cards;
use crate::parsers::{
    BackType, ConstructFileDataType, CustomData, NoteImportAction, Parseable,
    RenderOutputDirectoryType, TemplateData, get_cards, get_output_raw_dir,
};
use crate::schema::note::LinkedNote;
use indicatif::{ParallelProgressIterator, ProgressStyle};
use log::info;
use rayon::prelude::*;
use std::fs::{create_dir_all, read_to_string, write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct GenerateNoteFilesRequest {
    pub note_id: NoteId,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    /// If `None`, then won't be included in the generated files
    pub linked_notes: Option<Vec<LinkedNote>>,
    pub note_data: String,
    pub custom_data: CustomData,
}

#[derive(Clone, Debug)]
pub struct GenerateNoteFilesRequests {
    pub requests: Vec<GenerateNoteFilesRequest>,
    pub overridden_output_raw_dir: Option<PathBuf>,
    pub include_cards: bool,
    /// Create the `pdf` or other rendered format for the note and cards. This utilizes a cache to skip over previously rendered notes.
    pub render: bool,
    /// Skip the cache. Useful for when the rendering command was modified.
    pub force_render: bool,
}

#[derive(Clone, Debug, Copy, PartialEq)]
pub enum CardSide {
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderOutputType {
    Note,
    /// Card order
    Card(usize, CardSide),
}

pub fn file_in_cache(
    render: bool,
    force_render: bool,
    output_raw_filepath: &Path,
    output_rendered_filepath: &Path,
    note_file_data: &str,
    note_id: NoteId,
    line_to_hash: impl Fn(&str) -> Option<String>,
) -> Result<bool, Error> {
    if !force_render
        && output_raw_filepath.exists()
        && (!render || output_rendered_filepath.exists())
    {
        let current_raw_string = read_to_string(output_raw_filepath).map_err(|e| Error::Io {
            description: format!("Failed to read {}", &output_raw_filepath.display()),
            source: e,
        })?;
        let current_raw_lines = current_raw_string.lines().collect::<Vec<_>>();
        let current_rendered_hash_opt = current_raw_lines.last().and_then(|x| line_to_hash(x));
        if let Some(current_rendered_hash) = current_rendered_hash_opt {
            let new_note_data_hash = sha256::digest(note_file_data);
            if current_rendered_hash == new_note_data_hash {
                // Also check that the current note data has the correct hash. This is because locally editing the notes file will cause the data to change, but not the hash. This ensures that syncing local files to the database rerenders the file if it was updated.
                // Remove last line to get current note file data
                let current_note_file_data = format!(
                    "{}\n",
                    current_raw_lines[..current_raw_lines.len().saturating_sub(1)].join("\n")
                );
                let current_note_data_hash = sha256::digest(current_note_file_data);
                if current_rendered_hash == current_note_data_hash {
                    info!(
                        "[Note Id: {}] Existing file is up to date. Skipping.",
                        note_id
                    );
                    return Ok(true);
                }
                info!(
                    "[Note Id: {}] Hash matches, but current file is not up to date, so generating files.",
                    note_id
                );
            }
        }
    }
    Ok(false)
}

// There is no reason to only render a single card from a note. Any changes to 1 card likely change the entire note, so other cards change as well. Thus, this function parses the entire note and all its cards.
/// This creates the corresponding files for all of the notes' cards as well.
/// It creates the following files:
/// - a raw and rendered version of the notes in the notes directory
/// - (optional) a raw and rendered version of each card in the cards
///
/// Returns the file path of the raw note file.
pub fn create_note_files_bulk(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequests,
) -> Result<Vec<Result<PathBuf, Error>>, Error> {
    // Get template
    let TemplateData {
        template_contents,
        body_placeholder,
    } = parser.template_contents().map_err(|e| Error::Io {
        description: format!(
            "Failed to read template for parser {}",
            &parser.get_parser_name()
        ),
        source: e,
    })?;

    // let total_notes = request.requests.len();
    // let counter = Arc::new(AtomicUsize::new(0));
    info!(
        "Generating note files for the `{}` parser",
        parser.get_parser_name()
    );
    let style = ProgressStyle::with_template(
        "[{elapsed_precise}] ETA: {eta} {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");
    // request.requests.len() as u64
    let result = request
        .requests
        // .iter()
        .par_iter()
        .progress_with_style(style)
        .map(|create_note_files_request| {
            create_note_files(
                parser,
                create_note_files_request,
                template_contents.as_str(),
                body_placeholder.as_str(),
                request.overridden_output_raw_dir.as_deref(),
                request.include_cards,
                request.render,
                request.force_render,
                // &Arc::clone(&counter),
                // total_notes,
            )
        })
        .collect();
    Ok(result)
}

#[allow(clippy::too_many_arguments, reason = "function is only called once")]
#[allow(clippy::too_many_lines, reason = "off by a few")]
fn create_note_files(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequest,
    template_contents: &str,
    body_placeholder: &str,
    overridden_output_raw_dir: Option<&Path>,
    include_cards: bool,
    render: bool,
    force_render: bool,
    // _counter: &Arc<AtomicUsize>,
    // _total_notes: usize,
) -> Result<PathBuf, Error> {
    let start = Instant::now();

    let GenerateNoteFilesRequest {
        note_id, note_data, ..
    } = request;

    let note_file_data = parser.construct_full_file_data(
        &[(ConstructFileDataType::Note, request)],
        &NoteImportAction::Update(0),
    );
    let output_rendered_filename = parser.get_output_filename(RenderOutputType::Note, *note_id);
    let mut output_text_filepath = get_output_raw_dir(
        parser.get_parser_name(),
        RenderOutputType::Note,
        overridden_output_raw_dir,
    );
    output_text_filepath.push(&output_rendered_filename);
    output_text_filepath.set_extension(parser.file_extension());
    let output_rendered_dir = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    let mut output_rendered_filepath = output_rendered_dir.clone();
    output_rendered_filepath.push(&output_rendered_filename);

    // Check cache
    let mut file_contents = template_contents.replace(body_placeholder, &note_file_data);
    let line_to_hash = |line: &str| -> Option<String> {
        parser
            .extract_comment(line)
            .strip_prefix("hash: ")
            .map(|x| x.to_string())
    };
    let in_cache = file_in_cache(
        render,
        force_render,
        &output_text_filepath,
        &output_rendered_filepath,
        &file_contents,
        *note_id,
        line_to_hash,
    )?;

    let result = if in_cache {
        Ok(output_text_filepath)
    } else {
        // Create card files
        if include_cards {
            let cards = get_cards(parser, None, note_data, false, true)?;
            cards
                .par_iter()
                .enumerate()
                .try_for_each(|(i, card)| -> Result<(), Error> {
                    let card_order = i + 1;
                    let card_file_data = parser.construct_full_file_data(
                        &[(
                            ConstructFileDataType::Card(card_order, card, CardSide::Front),
                            request,
                        )],
                        &NoteImportAction::Update(0),
                    );
                    let file_contents =
                        template_contents.replace(body_placeholder, &card_file_data);
                    create_raw_and_rendered_file(
                        parser,
                        *note_id,
                        ConstructFileDataType::Card(card_order, card, CardSide::Front),
                        &file_contents,
                        overridden_output_raw_dir,
                        render,
                    )?;

                    if matches!(card.back_type, BackType::OnlyAnswered) {
                        let card_file_data = parser.construct_full_file_data(
                            &[(
                                ConstructFileDataType::Card(card_order, card, CardSide::Back),
                                request,
                            )],
                            &NoteImportAction::Update(0),
                        );
                        let file_contents =
                            template_contents.replace(body_placeholder, &card_file_data);
                        create_raw_and_rendered_file(
                            parser,
                            *note_id,
                            ConstructFileDataType::Card(card_order, card, CardSide::Back),
                            &file_contents,
                            overridden_output_raw_dir,
                            render,
                        )?;
                    }
                    Ok(())
                })?;
        }

        // Create note files
        // Add hash to file if:
        // - it is a note
        // - the note was rendered OR the note was created somewhere else for comparison purposes
        if render || overridden_output_raw_dir.is_some() {
            let file_contents_hash = sha256::digest(&file_contents);
            let file_contents_hash_str = format!("hash: {}", file_contents_hash);
            let hash_line = parser.construct_comment(&file_contents_hash_str);
            file_contents.push_str(hash_line.as_str());
        }
        create_raw_and_rendered_file(
            parser,
            *note_id,
            ConstructFileDataType::Note,
            &file_contents,
            overridden_output_raw_dir,
            render,
        )
    };
    // let done_count = counter.fetch_add(1, Ordering::SeqCst) + 1;
    // info!("Parsed {} out of {} notes", done_count, total_notes);
    let duration = start.elapsed();
    info!("[Note Id: {}] Duration: {:?}", note_id, duration);
    result
}

fn create_raw_and_rendered_file(
    parser: &dyn Parseable,
    note_id: NoteId,
    output_type: ConstructFileDataType,
    file_contents: &str,
    overridden_output_raw_dir: Option<&Path>,
    render: bool,
) -> Result<PathBuf, Error> {
    let render_output_type: RenderOutputType = match output_type {
        ConstructFileDataType::Note => RenderOutputType::Note,
        ConstructFileDataType::Card(order, _, side) => RenderOutputType::Card(order, side),
    };

    // Write to raw file
    let output_rendered_filename = parser.get_output_filename(render_output_type, note_id);
    let mut output_text_filepath = get_output_raw_dir(
        parser.get_parser_name(),
        render_output_type,
        overridden_output_raw_dir,
    );
    output_text_filepath.push(&output_rendered_filename);
    output_text_filepath.set_extension(parser.file_extension());
    create_dir_all(output_text_filepath.parent().unwrap()).unwrap();

    // Warn before overwriting file
    // TODO: This doesn't work because the new file may just be the current file + linked notes. In this case we are not overwriting.
    // let output_text_filepath_current_data_opt = read_to_string(&output_text_filepath).ok();
    // if let Some(current_file_contents) = output_text_filepath_current_data_opt {
    //     if current_file_contents != file_contents {
    //         let prompt = format!(
    //             "Are you sure you want to overwrite {}?",
    //             output_text_filepath.display()
    //         );
    //         let ans = Confirm::new(&prompt).with_default(false).prompt();
    //         match ans {
    //             Ok(true) => {}
    //             Ok(false) | Err(_) => {
    //                 return Err(Error::Io {
    //                     description: format!(
    //                         "Aborting since the file {} already contains data.",
    //                         output_text_filepath.display()
    //                     ),
    //                     source: std::io::Error::new(
    //                         std::io::ErrorKind::AlreadyExists,
    //                         output_text_filepath.to_str().unwrap(),
    //                     ),
    //                 });
    //             }
    //         }
    //     }
    // }
    write(&output_text_filepath, file_contents).map_err(|e| Error::Io {
        description: format!(
            "[Note Id: {}] Failed to write to {}",
            note_id,
            &output_text_filepath.display()
        ),
        source: e,
    })?;

    if render {
        let directory_output_type = match render_output_type {
            RenderOutputType::Note => RenderOutputDirectoryType::Note,
            RenderOutputType::Card(..) => RenderOutputDirectoryType::Card,
        };
        let output_rendered_dir = parser.get_output_rendered_dir(directory_output_type);
        let mut output_rendered_filepath = output_rendered_dir.clone();
        output_rendered_filepath.push(&output_rendered_filename);

        let aux_dir = parser.get_aux_dir(render_output_type, note_id);

        // Render Image Occlusions
        match output_type {
            ConstructFileDataType::Note => {}
            ConstructFileDataType::Card(card_order, card_data, side) => {
                let mut image_occlusion_output_rendered_filepath =
                    get_image_occlusion_rendered_directory();
                image_occlusion_output_rendered_filepath.push(
                    parser.get_output_filename(RenderOutputType::Card(card_order, side), note_id),
                );
                create_image_occlusion_cards(
                    card_data,
                    side,
                    &image_occlusion_output_rendered_filepath,
                )?;
            }
        }

        let output = parser.render_file(
            &aux_dir,
            &output_text_filepath,
            &output_rendered_dir,
            &output_rendered_filepath,
        )?;

        if !output_rendered_filepath.exists() {
            dbg!(&output);
            return Err(Error::Io {
                description: format!(
                    "[Note Id: {}] Failed to read {}",
                    note_id,
                    output_rendered_filepath.display()
                ),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    output_rendered_filepath.to_str().unwrap(),
                ),
            });
        }
    }

    Ok(output_text_filepath)
}
