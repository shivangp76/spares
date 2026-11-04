use std::collections::HashMap;
use std::fs::File;
use std::fs::create_dir_all;
use std::fs::read_to_string;
use std::fs::remove_file;
use std::fs::rename;
use std::fs::write;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;

#[cfg(not(target_family = "wasm"))]
use indicatif::ParallelProgressIterator;
#[cfg(not(target_family = "wasm"))]
use indicatif::ProgressStyle;
use log::debug;
use log::info;
use log::warn;
use rayon::prelude::*;
use web_time::Instant;

use super::image_occlusion::get_image_occlusion_rendered_directory;
use crate::Error;
use crate::config::SparesExternalConfig;
use crate::config::get_cache_dir;
use crate::helpers::write_if_changed;
use crate::model::NoteId;
use crate::notes_vcs;
use crate::notes_vcs::note_write_dir;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ConstructFileDataType;
use crate::parsers::CustomData;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::TemplateType;
use crate::parsers::WebRenderMode;
use crate::parsers::get_cards;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::ImageOcclusionSources;
use crate::parsers::image_occlusion::create_image_occlusion_cards;
use crate::parsers::image_occlusion::read_image_occlusions;
use crate::parsers::render_dependencies_filepath;
use crate::parsers::web_render_filepath;
use crate::schema::note::LinkedNote;

/// Whether to compile `parser`'s notes and cards and write its cards' raw files. Parsers that
/// render in the browser skip these if [`SparesExternalConfig::render_browser_parsers`] is off.
/// A missing config uses the default.
pub fn generates_render_files(
    parser: &dyn Parseable,
    config: Option<&SparesExternalConfig>,
) -> bool {
    !parser.renders_in_browser() || config.is_none_or(|c| c.render_browser_parsers)
}

fn blake3_hex(data: impl AsRef<[u8]>) -> String {
    blake3::hash(data.as_ref()).to_hex().to_string()
}

/// What decides whether a note's rendered files are up to date, besides their existing: the hash
/// of its raw file, `file_contents`, and of the files its image occlusions are drawn from,
/// `image_occlusion_sources` (see [`image_occlusion_sources_hash`]). So replacing an image
/// occlusion's image or clozes file renders the note again, though its text is unchanged. Other
/// files a note reads, like a template it includes, are left out, so editing them renders
/// nothing again until the note changes or rendering is forced.
///
/// A note without image occlusions has the hash of its raw file, as before image occlusions were
/// part of it, so those notes aren't rendered again after upgrading.
fn render_key(file_contents: &str, image_occlusion_sources: Option<&str>) -> String {
    match image_occlusion_sources {
        None => blake3_hex(file_contents),
        Some(sources) => {
            let mut hasher = blake3::Hasher::new();
            hasher.update(file_contents.as_bytes());
            hasher.update(&[0]);
            hasher.update(sources.as_bytes());
            hasher.finalize().to_hex().to_string()
        }
    }
}

/// What a card's rendered file is made from, to render the card again only if it changed when
/// its note did: the hash of its raw file, `file_contents`, how web clients show it, and the
/// note's [`image_occlusion_sources_hash`], since a card's image occlusions are drawn from those
/// files but only name them. Like a note's [`render_key`], it leaves out other files the card
/// reads.
fn card_render_key(
    file_contents: &str,
    web_render_mode: WebRenderMode,
    image_occlusion_sources: Option<&str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(file_contents.as_bytes());
    hasher.update(&[0]);
    hasher.update(format!("{web_render_mode:?}").as_bytes());
    hasher.update(&[0]);
    hasher.update(image_occlusion_sources.unwrap_or_default().as_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Where the [`card_render_key`] of the card rendered to `output_rendered_filename` is kept: beside
/// the notes' render hashes, in the cache, so that, like them, it isn't synced and goes when note
/// ids are renumbered.
pub(crate) fn card_render_key_path(
    parser: &dyn Parseable,
    output_rendered_filename: &str,
) -> PathBuf {
    let stem = Path::new(output_rendered_filename)
        .file_stem()
        .unwrap_or_default();
    get_cache_dir()
        .join("note-hashes")
        .join(parser.get_parser_name())
        .join(stem)
}

/// The hash of the path and contents of each original image and clozes file of the image
/// occlusions in `note_data`, in order, for [`render_key`]. A file that can't be read is hashed as
/// missing, so the note is rendered again, and reports it, until it is back. `None` if the note
/// has no image occlusions, or they can't be read, in which case rendering reports why.
fn image_occlusion_sources_hash(parser: &dyn Parseable, note_data: &str) -> Option<String> {
    let image_occlusions = read_image_occlusions(note_data, parser, false).ok()?;
    if image_occlusions.is_empty() {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    for (_, image_occlusion) in &image_occlusions {
        for path in [
            &image_occlusion.original_image_filepath,
            &image_occlusion.clozes_filepath,
        ] {
            hasher.update(path.as_os_str().as_encoded_bytes());
            hasher.update(&[0]);
            match std::fs::read(path) {
                Ok(contents) => {
                    hasher.update(b"file");
                    hasher.update(blake3::hash(&contents).as_bytes());
                }
                Err(_) => {
                    hasher.update(b"missing");
                }
            }
        }
    }
    Some(hasher.finalize().to_hex().to_string())
}

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
    /// Pre-parsed cards per note. When present for a note, `create_note_files` uses them instead
    /// of re-parsing `note_data` (which would duplicate the parse already done by
    /// `add_order_to_note_data` in the update path). Falls back to `get_cards` for notes absent
    /// from the map.
    pub precomputed_cards: Option<HashMap<NoteId, Vec<CardData>>>,
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

#[expect(
    clippy::too_many_arguments,
    reason = "the render key needs the image occlusion files beside the note"
)]
pub fn file_in_cache(
    render: bool,
    force_render: bool,
    output_raw_filepath: &Path,
    output_rendered_filepath: &Path,
    note_file_data: &str,
    image_occlusion_sources: Option<&str>,
    note_id: NoteId,
    line_to_hash: impl Fn(&str) -> Option<String>,
) -> Result<bool, Error> {
    if !force_render
        && output_raw_filepath.exists()
        && (!render || output_rendered_filepath.exists())
    {
        // Read only the tail of the file to extract the last line's hash,
        // avoiding a full multi-MB read for the common cache-miss path.
        let current_rendered_hash_opt = {
            let mut file = File::open(output_raw_filepath).map_err(|e| Error::Io {
                description: format!("Failed to read {}", output_raw_filepath.display()),
                source: e,
            })?;
            let file_len = file
                .metadata()
                .map_err(|e| Error::Io {
                    description: format!(
                        "Failed to read metadata for {}",
                        output_raw_filepath.display()
                    ),
                    source: e,
                })?
                .len();
            file.seek(SeekFrom::Start(file_len.saturating_sub(512)))
                .map_err(|e| Error::Io {
                    description: format!("Failed to seek in {}", output_raw_filepath.display()),
                    source: e,
                })?;
            let mut tail_bytes = Vec::new();
            file.read_to_end(&mut tail_bytes).map_err(|e| Error::Io {
                description: format!("Failed to read {}", output_raw_filepath.display()),
                source: e,
            })?;
            // Get the last non-empty line, handling any trailing newlines.
            tail_bytes
                .rsplit(|&b| b == b'\n')
                .find(|line| !line.is_empty())
                .and_then(|line_bytes| std::str::from_utf8(line_bytes).ok())
                .and_then(line_to_hash)
        };

        if let Some(current_rendered_hash) = current_rendered_hash_opt {
            let new_note_data_hash = render_key(note_file_data, image_occlusion_sources);
            if current_rendered_hash == new_note_data_hash {
                // Also check that the current note data has the correct hash. This is because locally editing the notes file will cause the data to change, but not the hash. This ensures that syncing local files to the database rerenders the file if it was updated.
                // Full read only happens here, on the rare path where hashes match.
                let current_raw_string =
                    read_to_string(output_raw_filepath).map_err(|e| Error::Io {
                        description: format!("Failed to read {}", output_raw_filepath.display()),
                        source: e,
                    })?;
                // Use rfind to locate the last line boundary without collecting all lines
                // into a Vec or allocating a join buffer.
                let content_end = current_raw_string
                    .trim_end_matches('\n')
                    .rfind('\n')
                    .map_or(0, |i| i + 1);
                let current_note_data_hash =
                    render_key(&current_raw_string[..content_end], image_occlusion_sources);
                if current_rendered_hash == current_note_data_hash {
                    info!(
                        "[Note Id: {}] Existing file is up to date. Skipping.",
                        note_id.letters()
                    );
                    return Ok(true);
                }
                info!(
                    "[Note Id: {}] Hash matches, but current file is not up to date, so generating files.",
                    note_id.letters()
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
///
/// Does nothing in a browser, which has no files: the server writes them once the browser's
/// changes reach it (see [`crate::platform`]).
pub fn create_note_files_bulk(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequests,
) -> Result<Vec<Result<PathBuf, Error>>, Error> {
    if cfg!(target_family = "wasm") {
        return Ok(Vec::new());
    }
    // Get template
    let (note_template_contents, body_placeholder) = parser
        .get_template_data(TemplateType::Note)
        .map_err(|e| Error::Io {
        description: format!(
            "Failed to read template for parser {}",
            parser.get_parser_name()
        ),
        source: e,
    })?;
    let (card_template_contents, _) =
        parser
            .get_template_data(TemplateType::Card)
            .map_err(|e| Error::Io {
                description: format!(
                    "Failed to read template for parser {}",
                    parser.get_parser_name()
                ),
                source: e,
            })?;

    // let total_notes = request.requests.len();
    // let counter = Arc::new(AtomicUsize::new(0));
    info!(
        "Generating note files for the `{}` parser",
        parser.get_parser_name()
    );
    let requests = request.requests.par_iter();
    // A progress bar in the terminal
    #[cfg(not(target_family = "wasm"))]
    let requests = requests.progress_with_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] ETA: {eta} {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
        )
        .unwrap()
        .progress_chars("##-"),
    );
    // Rayon's threads render in the current collection too.
    let context = crate::collection::capture();
    let result = requests
        .map(|create_note_files_request| {
            let _collection = context.enter();
            create_note_files(
                parser,
                create_note_files_request,
                note_template_contents.as_str(),
                card_template_contents.as_str(),
                body_placeholder.as_str(),
                request.overridden_output_raw_dir.as_deref(),
                request.include_cards,
                request.render,
                request.force_render,
                request.precomputed_cards.as_ref(),
                // &Arc::clone(&counter),
                // total_notes,
            )
        })
        .collect();
    Ok(result)
}

/// The raw note file, without the hash line added when rendering.
pub fn construct_note_file_contents(
    parser: &dyn Parseable,
    note_template_contents: &str,
    body_placeholder: &str,
    request: &GenerateNoteFilesRequest,
) -> String {
    let note_file_data = parser.construct_full_file_data(
        &[(ConstructFileDataType::Note, request)],
        &NoteImportAction::Update(request.note_id),
    );
    note_template_contents.replace(body_placeholder, &note_file_data)
}

/// The raw file of one side of a card.
pub fn construct_card_file_contents(
    parser: &dyn Parseable,
    card_template_contents: &str,
    body_placeholder: &str,
    request: &GenerateNoteFilesRequest,
    card_order: usize,
    card: &CardData,
    side: CardSide,
) -> String {
    let card_file_data = parser.construct_full_file_data(
        &[(ConstructFileDataType::Card(card_order, card, side), request)],
        &NoteImportAction::Update(request.note_id),
    );
    card_template_contents.replace(body_placeholder, &card_file_data)
}

#[allow(clippy::too_many_arguments, reason = "function is only called once")]
#[expect(clippy::too_many_lines)]
fn create_note_files(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequest,
    note_template_contents: &str,
    card_template_contents: &str,
    body_placeholder: &str,
    overridden_output_raw_dir: Option<&Path>,
    include_cards: bool,
    render: bool,
    force_render: bool,
    precomputed_cards: Option<&HashMap<NoteId, Vec<CardData>>>,
    // _counter: &Arc<AtomicUsize>,
    // _total_notes: usize,
) -> Result<PathBuf, Error> {
    let start = Instant::now();

    let GenerateNoteFilesRequest {
        note_id,
        note_data,
        custom_data,
        ..
    } = request;
    let web_render_mode = WebRenderMode::from_custom_data(custom_data);

    let output_rendered_filename = parser.get_output_filename(RenderOutputType::Note, *note_id);
    let mut output_text_filepath =
        note_write_dir(parser.get_parser_name(), overridden_output_raw_dir);
    output_text_filepath.push(&output_rendered_filename);
    output_text_filepath.set_extension(parser.file_extension());
    // Files in the notes repository carry no hash line, which would change on every render and
    // conflict with users' edits. The hash of the content last compiled is kept beside the cache.
    let vcs_format = overridden_output_raw_dir.is_none() && notes_vcs::enabled();
    let output_rendered_dir = parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    let mut output_rendered_filepath = output_rendered_dir.clone();
    output_rendered_filepath.push(&output_rendered_filename);

    // Check cache
    let mut file_contents =
        construct_note_file_contents(parser, note_template_contents, body_placeholder, request);
    let line_to_hash = |line: &str| -> Option<String> {
        parser
            .extract_comment(line)
            .strip_prefix("hash: ")
            .map(|x| x.to_string())
    };
    // Read before the cards are made, so a file replaced while they are is read again next time
    let image_occlusion_sources = image_occlusion_sources_hash(parser, note_data);
    let image_occlusion_sources = image_occlusion_sources.as_deref();
    let in_cache = if vcs_format {
        render
            && !force_render
            && output_rendered_filepath.exists()
            && notes_vcs::read_render_hash(parser.get_parser_name(), *note_id)
                .is_some_and(|hash| hash == render_key(&file_contents, image_occlusion_sources))
            && std::fs::read(&output_text_filepath).is_ok_and(|raw| raw == file_contents.as_bytes())
    } else {
        file_in_cache(
            render,
            force_render,
            &output_text_filepath,
            &output_rendered_filepath,
            &file_contents,
            image_occlusion_sources,
            *note_id,
            line_to_hash,
        )?
    };

    let result = if in_cache {
        Ok(output_text_filepath)
    } else {
        // Create card files
        if include_cards {
            let cards = match precomputed_cards.and_then(|m| m.get(note_id)) {
                Some(cards) => cards.clone(),
                None => get_cards(parser, None, note_data, false, true)?,
            };
            // The note's sources, for the cards' render keys, before the name is reused below
            let sources_hash = image_occlusion_sources;
            let card_key = |file_contents: &str| {
                (!force_render)
                    .then(|| card_render_key(file_contents, web_render_mode, sources_hash))
            };
            // Read once for all of the note's cards, and only if their images are made. Overlays
            // are in the cards' sources instead.
            let image_occlusion_sources = ImageOcclusionSources::read(
                if render && !parser.image_occlusion_overlays() {
                    &cards
                } else {
                    &[]
                },
                parser.image_occlusion_overlays(),
            );
            let context = crate::collection::capture();
            cards
                .par_iter()
                .enumerate()
                .try_for_each(|(i, card)| -> Result<(), Error> {
                    let _collection = context.enter();
                    let card_order = i + 1;
                    let file_contents = construct_card_file_contents(
                        parser,
                        card_template_contents,
                        body_placeholder,
                        request,
                        card_order,
                        card,
                        CardSide::Front,
                    );
                    create_raw_and_rendered_file(
                        parser,
                        *note_id,
                        ConstructFileDataType::Card(card_order, card, CardSide::Front),
                        &file_contents,
                        overridden_output_raw_dir,
                        render,
                        web_render_mode,
                        Some(&image_occlusion_sources),
                        card_key(&file_contents).as_deref(),
                    )?;

                    if matches!(card.back_type, BackType::CardFilePath) {
                        let file_contents = construct_card_file_contents(
                            parser,
                            card_template_contents,
                            body_placeholder,
                            request,
                            card_order,
                            card,
                            CardSide::Back,
                        );
                        create_raw_and_rendered_file(
                            parser,
                            *note_id,
                            ConstructFileDataType::Card(card_order, card, CardSide::Back),
                            &file_contents,
                            overridden_output_raw_dir,
                            render,
                            web_render_mode,
                            Some(&image_occlusion_sources),
                            card_key(&file_contents).as_deref(),
                        )?;
                    }
                    Ok(())
                })?;
        }

        // Create note files
        // Add hash to file if:
        // - it is a note
        // - the note was rendered OR the note was created somewhere else for comparison purposes
        let file_contents_hash = render_key(&file_contents, image_occlusion_sources);
        if !vcs_format && (render || overridden_output_raw_dir.is_some()) {
            let file_contents_hash_str = format!("hash: {}", file_contents_hash);
            let hash_line = parser.construct_comment(&file_contents_hash_str);
            file_contents.push_str(hash_line.as_str());
        }
        let result = create_raw_and_rendered_file(
            parser,
            *note_id,
            ConstructFileDataType::Note,
            &file_contents,
            overridden_output_raw_dir,
            render,
            web_render_mode,
            None,
            None,
        );
        if vcs_format {
            if render && result.is_ok() && output_rendered_filepath.exists() {
                notes_vcs::write_render_hash(
                    parser.get_parser_name(),
                    *note_id,
                    &file_contents_hash,
                );
            } else if result.is_err() {
                notes_vcs::remove_render_hash(parser.get_parser_name(), *note_id);
            }
        }
        result
    };
    // let done_count = counter.fetch_add(1, Ordering::SeqCst) + 1;
    // info!("Parsed {} out of {} notes", done_count, total_notes);
    let duration = start.elapsed();
    debug!("[Note Id: {}] Duration: {:?}", note_id.letters(), duration);
    result
}

#[allow(clippy::too_many_arguments)]
#[expect(clippy::too_many_lines)]
fn create_raw_and_rendered_file(
    parser: &dyn Parseable,
    note_id: NoteId,
    output_type: ConstructFileDataType,
    file_contents: &str,
    overridden_output_raw_dir: Option<&Path>,
    render: bool,
    web_render_mode: WebRenderMode,
    // What a card's image occlusions are drawn from, read for the note's cards
    image_occlusion_sources: Option<&ImageOcclusionSources>,
    // A card's `card_render_key`, to render it only if it changed. `None` for notes, which have
    // their own cache, and when rendering is forced.
    card_render_key: Option<&str>,
) -> Result<PathBuf, Error> {
    let render_output_type: RenderOutputType = match output_type {
        ConstructFileDataType::Note => RenderOutputType::Note,
        ConstructFileDataType::Card(order, _, side) => RenderOutputType::Card(order, side),
    };

    // CLI cards are reviewed via an external command and have no rendered
    // document. If this content carries a CLI block, skip rendering entirely
    // (no pandoc/XeLaTeX run, no placeholder file needed). The raw text file
    // is still written so future `sync` operations can re-import the note.
    let render = render && parser.get_cli_blocks(file_contents)?.is_empty();

    // Write to raw file
    let output_rendered_filename = parser.get_output_filename(render_output_type, note_id);
    let mut output_text_filepath = match render_output_type {
        RenderOutputType::Note => {
            note_write_dir(parser.get_parser_name(), overridden_output_raw_dir)
        }
        RenderOutputType::Card(..) => get_output_raw_dir(
            parser.get_parser_name(),
            render_output_type,
            overridden_output_raw_dir,
        ),
    };
    output_text_filepath.push(&output_rendered_filename);
    output_text_filepath.set_extension(parser.file_extension());
    create_dir_all(output_text_filepath.parent().unwrap()).unwrap();

    // Note we cannot warn before overwriting the file since the new file may just be the current file with linked notes. In this case, we are not overwriting.
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
    // A raw file that would be written with what it holds keeps its modification time
    let raw_changed =
        write_if_changed(&output_text_filepath, file_contents).map_err(|e| Error::Io {
            description: format!(
                "[Note Id: {}] Failed to write to {}",
                note_id.letters(),
                output_text_filepath.display()
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

        // A card of a note that changed is rendered again only if it changed too
        let key_path =
            card_render_key.map(|_| card_render_key_path(parser, &output_rendered_filename));
        if let (Some(key), Some(key_path)) = (card_render_key, &key_path)
            && card_is_rendered(key, key_path, raw_changed, &output_rendered_filepath)
        {
            return Ok(output_text_filepath);
        }

        let aux_dir = parser.get_aux_dir(render_output_type, note_id);

        // Render Image Occlusions, unless the card's source has them as overlays
        match output_type {
            ConstructFileDataType::Note => {}
            ConstructFileDataType::Card(..) if parser.image_occlusion_overlays() => {}
            ConstructFileDataType::Card(card_order, card_data, side) => {
                let mut image_occlusion_output_rendered_filepath =
                    get_image_occlusion_rendered_directory();
                image_occlusion_output_rendered_filepath.push(
                    parser.get_output_filename(RenderOutputType::Card(card_order, side), note_id),
                );
                // Read for this card alone if they weren't for the note's cards
                let read;
                let image_occlusion_sources = if let Some(sources) = image_occlusion_sources {
                    sources
                } else {
                    read = ImageOcclusionSources::read(
                        std::slice::from_ref(card_data),
                        parser.image_occlusion_overlays(),
                    );
                    &read
                };
                create_image_occlusion_cards(
                    card_data,
                    side,
                    &image_occlusion_output_rendered_filepath,
                    image_occlusion_sources,
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
            return Err(Error::Io {
                description: format!(
                    "[Note Id: {}] Failed to read {}: {:?}",
                    note_id.letters(),
                    output_rendered_filepath.display(),
                    output
                ),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    output_rendered_filepath.to_str().unwrap(),
                ),
            });
        }

        add_audio_dependencies(parser, note_id, file_contents, &output_rendered_filepath);
        if matches!(render_output_type, RenderOutputType::Note) {
            copy_rendered_note(parser, note_id, &output_rendered_filepath);
        }

        if parser.has_web_render() {
            create_web_render_file(
                parser,
                note_id,
                &output_text_filepath,
                &output_rendered_filepath,
                web_render_mode,
            );
        }

        if let (Some(key), Some(key_path)) = (card_render_key, &key_path) {
            let result = key_path
                .parent()
                .map_or(Ok(()), create_dir_all)
                .and_then(|()| write(key_path, key));
            if let Err(e) = result {
                warn!("Failed to write {}: {e}", key_path.display());
            }
        }
    }

    Ok(output_text_filepath)
}

/// Whether a card's rendered file, `output_rendered_filepath`, is still what rendering it would
/// make: its raw file did not change (`raw_changed`), and `key_path` holds its [`card_render_key`],
/// `key`, from when it was last rendered. If not, the key is removed until it is rendered again,
/// so that a render that fails does not leave it behind.
fn card_is_rendered(
    key: &str,
    key_path: &Path,
    raw_changed: bool,
    output_rendered_filepath: &Path,
) -> bool {
    if !raw_changed
        && output_rendered_filepath.exists()
        && std::fs::read_to_string(key_path).is_ok_and(|old| old == key)
    {
        debug!(
            "{} is up to date. Skipping.",
            output_rendered_filepath.display()
        );
        return true;
    }
    let _ = remove_file(key_path);
    false
}

/// Puts a note's rendered file into the parser's directory in
/// [`crate::render::RenderConfig::copy_notes_to`], if it has one, as a hard link to it rather than
/// a copy, so that each later render writes the PDF there as it writes it here. It is linked beside
/// its destination and renamed into place, replacing what an editor wrote there. On another
/// filesystem, where it can't be linked, it is copied. Never fails the render.
fn copy_rendered_note(parser: &dyn Parseable, note_id: NoteId, rendered: &Path) {
    let config = crate::render::config();
    let Some(dir) = config.copy_notes_to.get(parser.get_parser_name()) else {
        return;
    };
    if let Err(e) = link_into(rendered, dir) {
        warn!(
            "[Note Id: {}] Failed to put {} into {}: {e}",
            note_id.letters(),
            rendered.display(),
            dir.display()
        );
    }
}

fn link_into(file: &Path, dir: &Path) -> std::io::Result<()> {
    let name = file
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?;
    let destination = dir.join(name);
    if is_same_file(file, &destination) {
        return Ok(());
    }
    let tmp = dir.join(format!(".{}.spares-tmp", name.to_string_lossy()));
    let _ = remove_file(&tmp);
    let result = create_dir_all(dir)
        .and_then(|()| {
            std::fs::hard_link(file, &tmp).or_else(|_| std::fs::copy(file, &tmp).map(|_| ()))
        })
        .and_then(|()| rename(&tmp, &destination));
    if result.is_err() {
        let _ = remove_file(&tmp);
    }
    result
}

/// Whether `a` and `b` are links to one file.
fn is_same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// Adds the audio clips `file_contents` links to the dependencies of its rendered file, since
/// renderers do not read them, so they are shipped like the files a render reads.
fn add_audio_dependencies(
    parser: &dyn Parseable,
    note_id: NoteId,
    file_contents: &str,
    output_rendered_filepath: &Path,
) {
    let clips = parser
        .audio_refs(file_contents)
        .into_iter()
        .map(|audio_ref| PathBuf::from(audio_ref.path))
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    if clips.is_empty() {
        return;
    }
    let dependencies_filepath = render_dependencies_filepath(output_rendered_filepath);
    if let Err(e) = RenderDependencies::extend_file(&dependencies_filepath, clips) {
        warn!(
            "[Note Id: {}] Failed to add audio clips to {}: {e}",
            note_id.letters(),
            dependencies_filepath.display()
        );
    }
}

/// Writes the HTML for web clients next to the rendered file, or an empty file if they should show
/// the rendered file instead (see [`web_render_filepath`]). Never fails the render, since the
/// rendered file is always there to fall back to.
fn create_web_render_file(
    parser: &dyn Parseable,
    note_id: NoteId,
    output_text_filepath: &Path,
    output_rendered_filepath: &Path,
    mode: WebRenderMode,
) {
    let output_web_filepath = web_render_filepath(output_rendered_filepath);
    let tmp_filepath = output_web_filepath.with_extension("html.tmp");
    let result = match mode {
        WebRenderMode::Pdf => Err(None),
        WebRenderMode::Auto | WebRenderMode::Html => parser
            .render_web_file(
                output_text_filepath,
                output_rendered_filepath,
                &tmp_filepath,
                mode,
            )
            .map_err(Some)
            .and_then(|()| {
                rename(&tmp_filepath, &output_web_filepath)
                    .map_err(|e| Some(format!("failed to move the HTML into place: {e}")))
            }),
    };
    if let Err(reason) = result {
        let _ = remove_file(&tmp_filepath);
        if let Some(reason) = reason {
            warn!(
                "[Note Id: {}] Web uses the rendered file for {}: {reason}",
                note_id.letters(),
                output_rendered_filepath.display()
            );
        }
        if let Err(e) = write(&output_web_filepath, "") {
            warn!(
                "[Note Id: {}] Failed to write {}: {e}",
                note_id.letters(),
                output_web_filepath.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::metadata;
    use std::fs::read_to_string;
    use std::fs::remove_file;
    use std::fs::write;
    use std::path::Path;

    use super::GenerateNoteFilesRequest;
    use super::GenerateNoteFilesRequests;
    use super::add_audio_dependencies;
    use super::blake3_hex;
    use super::card_is_rendered;
    use super::card_render_key;
    use super::create_note_files_bulk;
    use super::create_web_render_file;
    use super::generates_render_files;
    use super::image_occlusion_sources_hash;
    use super::link_into;
    use super::render_key;
    use crate::config::SparesExternalConfig;
    use crate::parsers::CustomData;
    use crate::parsers::WebRenderMode;
    use crate::parsers::has_web_render_file;
    use crate::parsers::impls::markdown::MarkdownParser;
    use crate::parsers::impls::typst::TypstParser;

    #[test]
    fn test_generates_render_files() {
        let browser = TypstParser::new();
        let local = MarkdownParser::new();
        let mut config = SparesExternalConfig::default();
        assert!(generates_render_files(&browser, Some(&config)));
        assert!(generates_render_files(&browser, None));

        config.render_browser_parsers = false;
        assert!(!generates_render_files(&browser, Some(&config)));
        // Parsers rendered on the server are unaffected.
        assert!(generates_render_files(&local, Some(&config)));
    }

    #[test]
    fn test_create_web_render_file_tombstones() {
        let dir = std::env::temp_dir().join(format!("spares-web-tombstone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parser = MarkdownParser::new();
        let text = dir.join("0001.md");
        let rendered = dir.join("0001.pdf");
        let html = dir.join("0001.html");
        let tikz = "\\begin{tikzpicture}\n\\end{tikzpicture}\n";
        let render = |contents: &str, mode| {
            write(&text, contents).unwrap();
            create_web_render_file(&parser, "aaa".parse().unwrap(), &text, &rendered, mode);
        };

        render("Text", WebRenderMode::Auto);
        assert!(has_web_render_file(&rendered));
        // A later render that decides on the rendered file replaces the HTML with an empty file
        render("Text", WebRenderMode::Pdf);
        assert_eq!(metadata(&html).unwrap().len(), 0);
        assert!(!has_web_render_file(&rendered));

        render(tikz, WebRenderMode::Html);
        assert!(has_web_render_file(&rendered));
        render(tikz, WebRenderMode::Auto);
        assert!(!has_web_render_file(&rendered));
        assert!(html.exists());
        assert!(!dir.join("0001.html.tmp").exists());

        // A failed render, here from a missing image, also leaves an empty file
        render("Text", WebRenderMode::Auto);
        render("![a](/nonexistent/spares/a.png)", WebRenderMode::Html);
        assert!(!has_web_render_file(&rendered));
    }

    #[test]
    fn test_web_render_mode_from_custom_data() {
        let mode =
            |value: serde_json::Value| WebRenderMode::from_custom_data(value.as_object().unwrap());
        assert_eq!(mode(serde_json::json!({})), WebRenderMode::Auto);
        assert_eq!(
            mode(serde_json::json!({"render": "pdf"})),
            WebRenderMode::Pdf
        );
        assert_eq!(
            mode(serde_json::json!({"render": "html"})),
            WebRenderMode::Html
        );
        assert_eq!(
            mode(serde_json::json!({"render": "other"})),
            WebRenderMode::Auto
        );
        assert_eq!(mode(serde_json::json!({"render": 1})), WebRenderMode::Auto);
    }

    #[test]
    fn test_add_audio_dependencies() {
        use crate::parsers::RenderDependencies;
        use crate::parsers::render_dependencies_filepath;

        let dir = std::env::temp_dir().join(format!("spares-audio-deps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rendered = dir.join("0001.pdf");
        let deps = render_dependencies_filepath(&rendered);
        write(&deps, r#"{"files":["/m/figure.png"],"packages":[]}"#).unwrap();
        let note_id = "aaa".parse().unwrap();
        add_audio_dependencies(
            &MarkdownParser::new(),
            note_id,
            "[🔊](/m/b.mp3) [🔊](rel.mp3) {{ [🔊](/m/a.ogg) }}",
            &rendered,
        );
        let dependencies: RenderDependencies =
            serde_json::from_str(&std::fs::read_to_string(&deps).unwrap()).unwrap();
        assert_eq!(
            dependencies.files,
            ["/m/a.ogg", "/m/b.mp3", "/m/figure.png"].map(std::path::PathBuf::from)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note with an image occlusion drawn from `image` and `clozes`, then `rest`
    fn image_occlusion_note(image: &Path, clozes: &Path, rest: &str) -> String {
        format!(
            "a\n<!--- spares: image occlusion start --->\n\
             <!--- original_image_filepath = \"{}\" --->\n\
             <!--- clozes_filepath = \"{}\" --->\n\
             ![image]({})\n\
             <!--- spares: image occlusion end --->\n{rest}",
            image.display(),
            clozes.display(),
            image.display(),
        )
    }

    #[test]
    fn test_render_key_follows_image_occlusion_files() {
        // In the collection, where image occlusion files must be
        let dir = crate::parsers::image_occlusion::get_image_occlusion_directory()
            .join(format!("spares-render-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (image, clozes) = (dir.join("a.png"), dir.join("a_clozes.svg"));
        write(&image, "image").unwrap();
        write(&clozes, "clozes").unwrap();
        let parser = MarkdownParser::new();
        let note = image_occlusion_note(&image, &clozes, "b");
        let key = || {
            render_key(
                &note,
                image_occlusion_sources_hash(&parser, &note).as_deref(),
            )
        };

        // A note without image occlusions keeps the hash it had before they were in the key
        assert_eq!(image_occlusion_sources_hash(&parser, "a {{b}}"), None);
        assert_eq!(render_key("a {{b}}", None), blake3_hex("a {{b}}"));

        let first = key();
        assert_ne!(first, blake3_hex(&note));
        assert_eq!(key(), first);
        // Replacing either file, at the same path, changes the key
        write(&image, "new image").unwrap();
        let replaced_image = key();
        assert_ne!(replaced_image, first);
        write(&clozes, "new clozes").unwrap();
        let replaced_clozes = key();
        assert_ne!(replaced_clozes, replaced_image);
        // As does a missing file, and its return
        std::fs::remove_file(&image).unwrap();
        assert_ne!(key(), replaced_clozes);
        write(&image, "new image").unwrap();
        assert_eq!(key(), replaced_clozes);
        // Other files the note reads are not in the key
        write(dir.join("header.tex"), "\\newcommand{\\x}{y}").unwrap();
        assert_eq!(key(), replaced_clozes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_note_files_are_made_again_when_an_image_occlusion_file_is_replaced() {
        // In the collection, where image occlusion files must be
        let dir = crate::parsers::image_occlusion::get_image_occlusion_directory()
            .join(format!("spares-io-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let (image, clozes) = (dir.join("a.png"), dir.join("a_clozes.svg"));
        write(&image, "image").unwrap();
        write(&clozes, "clozes").unwrap();
        let parser = MarkdownParser::new();
        let request = GenerateNoteFilesRequests {
            requests: vec![GenerateNoteFilesRequest {
                note_id: "aaa".parse().unwrap(),
                keywords: Vec::new(),
                tags: Vec::new(),
                linked_notes: None,
                note_data: image_occlusion_note(&image, &clozes, "b"),
                custom_data: CustomData::new(),
            }],
            overridden_output_raw_dir: Some(out.clone()),
            include_cards: false,
            render: false,
            force_render: false,
            precomputed_cards: None,
        };
        // The raw note file, whose last line is its render key
        let make = || {
            let paths = create_note_files_bulk(&parser, &request).unwrap();
            let path = paths.into_iter().next().unwrap().unwrap();
            let raw = read_to_string(&path).unwrap();
            let modified = metadata(&path).unwrap().modified().unwrap();
            (raw, modified)
        };

        let (first, first_modified) = make();
        let (again, again_modified) = make();
        assert_eq!(again, first);
        assert_eq!(
            again_modified, first_modified,
            "an unchanged note is skipped"
        );

        write(&image, "replaced").unwrap();
        let (replaced, _) = make();
        assert_ne!(replaced, first, "the note is made again with a new key");
        assert_eq!(
            replaced.lines().count(),
            first.lines().count(),
            "only the key changes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_card_render_key_follows_what_a_card_is_rendered_from() {
        let key = card_render_key("card", WebRenderMode::Auto, None);
        assert_eq!(card_render_key("card", WebRenderMode::Auto, None), key);
        for other in [
            card_render_key("card 2", WebRenderMode::Auto, None),
            card_render_key("card", WebRenderMode::Pdf, None),
            card_render_key("card", WebRenderMode::Auto, Some("sources")),
        ] {
            assert_ne!(other, key);
        }
    }

    #[test]
    fn test_a_card_is_rendered_again_only_if_it_changed() {
        let dir = std::env::temp_dir().join(format!("spares-card-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (key_path, rendered) = (dir.join("aaa-1-front"), dir.join("aaa-1-front.pdf"));
        let key = card_render_key("card", WebRenderMode::Auto, None);

        // Never rendered
        assert!(!card_is_rendered(&key, &key_path, false, &rendered));
        write(&rendered, "pdf").unwrap();
        write(&key_path, &key).unwrap();
        assert!(card_is_rendered(&key, &key_path, false, &rendered));
        // A changed raw file, or another key, renders it again, and the key goes until then
        assert!(!card_is_rendered(&key, &key_path, true, &rendered));
        assert!(!key_path.exists());
        write(&key_path, &key).unwrap();
        let other = card_render_key("card 2", WebRenderMode::Auto, None);
        assert!(!card_is_rendered(&other, &key_path, false, &rendered));
        assert!(!key_path.exists());
        // As does a missing rendered file
        write(&key_path, &key).unwrap();
        std::fs::remove_file(&rendered).unwrap();
        assert!(!card_is_rendered(&key, &key_path, false, &rendered));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_into_puts_the_rendered_file_there_whole() {
        let dir = std::env::temp_dir().join(format!("spares-link-into-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rendered = dir.join("abc.pdf");
        let out = dir.join("out/typst");
        write(&rendered, "new").unwrap();
        link_into(&rendered, &out).unwrap();
        assert_eq!(read_to_string(out.join("abc.pdf")).unwrap(), "new");
        // A render writing the file in place writes it there too.
        write(&rendered, "newer").unwrap();
        assert_eq!(read_to_string(out.join("abc.pdf")).unwrap(), "newer");
        link_into(&rendered, &out).unwrap();
        // One replacing it is linked again, over what an editor wrote.
        remove_file(&rendered).unwrap();
        write(&rendered, "newest").unwrap();
        write(out.join("abc.pdf"), "editor's").unwrap();
        link_into(&rendered, &out).unwrap();
        assert_eq!(read_to_string(out.join("abc.pdf")).unwrap(), "newest");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
