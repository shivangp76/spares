use std::collections::HashMap;
use std::fs::create_dir_all;
use std::fs::remove_file;
use std::fs::rename;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

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
use crate::helpers::write_if_changed;
use crate::model::NoteId;
use crate::notes_vcs::note_write_dir;
use crate::parsers::BackType;
use crate::parsers::CardData;
use crate::parsers::ConstructFileDataType;
use crate::parsers::CustomData;
use crate::parsers::NoteImportAction;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::RenderedFile;
use crate::parsers::TemplateType;
use crate::parsers::WebRenderMode;
use crate::parsers::get_cards;
use crate::parsers::get_output_raw_dir;
use crate::parsers::image_occlusion::ImageOcclusionSources;
use crate::parsers::image_occlusion::create_image_occlusion_cards;
use crate::parsers::image_occlusion::read_image_occlusions;
use crate::parsers::notes::NotePart;
use crate::parsers::render_reuse::RenderReuse;
use crate::parsers::render_reuse::render_aside;
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
    /// The notes' [`RenderStates`] from when they were last rendered here, to skip what is up to
    /// date. A note without any is rendered.
    pub render_states: HashMap<NoteId, RenderStates>,
}

#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash)]
pub enum CardSide {
    Front,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderOutputType {
    Note,
    /// Card order
    Card(usize, CardSide),
}

/// What rendering one of a note's outputs on this machine made, kept in the database (see
/// `api::render_state`). It isn't synced: each machine renders its own files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderState {
    /// What the rendered file was made from ([`render_key`] or [`card_render_key`]). The file is
    /// up to date while it exists and rendering it would have the same key.
    pub key: String,
    pub dependencies: RenderDependencies,
}

/// A note's [`RenderState`]s, by output
pub type RenderStates = HashMap<RenderOutputType, RenderState>;

/// What creating a note's files changed in its [`RenderStates`], for the caller to store
#[derive(Debug, Default)]
pub struct RenderStateChanges {
    /// Outputs rendered again
    pub rendered: Vec<(RenderOutputType, RenderState)>,
    /// Outputs that failed to render, so that they're rendered again, or that the note no longer
    /// has
    pub removed: Vec<RenderOutputType>,
}

/// One note's files, made by [`create_note_files_bulk`]
#[derive(Debug)]
pub struct NoteFiles {
    pub note_id: NoteId,
    /// The raw note file
    pub result: Result<PathBuf, Error>,
    /// Recorded even if some of the note's files failed
    pub render_states: RenderStateChanges,
}

// There is no reason to only render a single card from a note. Any changes to 1 card likely change the entire note, so other cards change as well. Thus, this function parses the entire note and all its cards.
/// This creates the corresponding files for all of the notes' cards as well.
/// It creates the following files:
/// - a raw and rendered version of the notes in the notes directory
/// - (optional) a raw and rendered version of each card in the cards
///
/// Returns each note's raw file and what changed in its [`RenderStates`], which the caller stores.
///
/// Does nothing in a browser, which has no files: the server writes them once the browser's
/// changes reach it (see [`crate::platform`]).
pub fn create_note_files_bulk(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequests,
) -> Result<Vec<NoteFiles>, Error> {
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
    // Cards the batch's notes had, to copy those whose source moved to another note or card
    let reuse = if request.render && !request.force_render {
        RenderReuse::snapshot(parser, &request.render_states)
    } else {
        None
    };
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
                request
                    .render_states
                    .get(&create_note_files_request.note_id),
                reuse.as_ref(),
            )
        })
        .collect();
    Ok(result)
}

/// The raw note file.
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
    render_states: Option<&RenderStates>,
    reuse: Option<&RenderReuse>,
) -> NoteFiles {
    let start = Instant::now();
    let changes = Mutex::new(RenderStateChanges::default());
    let result = write_note_files(
        parser,
        request,
        NoteTemplates {
            note: note_template_contents,
            card: card_template_contents,
            body_placeholder,
        },
        overridden_output_raw_dir,
        NoteFilesOptions {
            include_cards,
            render,
            force_render,
        },
        precomputed_cards,
        render_states,
        reuse,
        &changes,
    );
    debug!(
        "[Note Id: {}] Duration: {:?}",
        request.note_id.letters(),
        start.elapsed()
    );
    NoteFiles {
        note_id: request.note_id,
        result,
        render_states: changes
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    }
}

#[derive(Clone, Copy)]
struct NoteTemplates<'a> {
    note: &'a str,
    card: &'a str,
    body_placeholder: &'a str,
}

#[derive(Clone, Copy)]
struct NoteFilesOptions {
    include_cards: bool,
    render: bool,
    force_render: bool,
}

/// [`create_note_files`], recording what it renders in `changes`
#[expect(clippy::too_many_arguments)]
#[expect(clippy::too_many_lines)]
fn write_note_files(
    parser: &dyn Parseable,
    request: &GenerateNoteFilesRequest,
    templates: NoteTemplates<'_>,
    overridden_output_raw_dir: Option<&Path>,
    options: NoteFilesOptions,
    precomputed_cards: Option<&HashMap<NoteId, Vec<CardData>>>,
    render_states: Option<&RenderStates>,
    reuse: Option<&RenderReuse>,
    changes: &Mutex<RenderStateChanges>,
) -> Result<PathBuf, Error> {
    let NoteFilesOptions {
        include_cards,
        render,
        force_render,
    } = options;
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
    let mut output_rendered_filepath =
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
    output_rendered_filepath.push(&output_rendered_filename);

    // Check cache
    let file_contents =
        construct_note_file_contents(parser, templates.note, templates.body_placeholder, request);
    // Read before the cards are made, so a file replaced while they are is read again next time
    let image_occlusion_sources = image_occlusion_sources_hash(parser, note_data);
    let image_occlusion_sources = image_occlusion_sources.as_deref();
    let note_key = render_key(&file_contents, image_occlusion_sources);
    let stored = |output| {
        render_states
            .filter(|_| !force_render)
            .and_then(|states| states.get(&output))
    };
    // The raw file is also compared, since one edited by hand is rendered again when `spares sync`
    // imports it
    let in_cache = render
        && output_rendered_filepath.exists()
        && stored(RenderOutputType::Note).is_some_and(|state| state.key == note_key)
        && std::fs::read(&output_text_filepath).is_ok_and(|raw| raw == file_contents.as_bytes());
    if in_cache {
        info!(
            "[Note Id: {}] Existing file is up to date. Skipping.",
            note_id.letters()
        );
        return Ok(output_text_filepath);
    }

    let record = |output, result: Result<(PathBuf, Option<RenderState>), Error>| {
        let mut changes = crate::collection::lock(changes);
        match result {
            Ok((path, state)) => {
                changes.rendered.extend(state.map(|state| (output, state)));
                Ok(path)
            }
            Err(e) => {
                changes.removed.push(output);
                Err(e)
            }
        }
    };

    // Create card files
    if include_cards {
        let cards = match precomputed_cards.and_then(|m| m.get(note_id)) {
            Some(cards) => cards.clone(),
            None => get_cards(parser, None, note_data, false, true)?,
        };
        let sides = |card: &CardData| {
            if matches!(card.back_type, BackType::CardFilePath) {
                &[CardSide::Front, CardSide::Back][..]
            } else {
                &[CardSide::Front][..]
            }
        };
        // Read once for all of the note's cards, and only if their images are made. Overlays
        // are in the cards' sources instead.
        let card_image_occlusion_sources = ImageOcclusionSources::read(
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
                for &side in sides(card) {
                    let file_contents = construct_card_file_contents(
                        parser,
                        templates.card,
                        templates.body_placeholder,
                        request,
                        card_order,
                        card,
                        side,
                    );
                    let output = RenderOutputType::Card(card_order, side);
                    // A card of a note that changed is rendered again only if it changed too
                    let key =
                        card_render_key(&file_contents, web_render_mode, image_occlusion_sources);
                    record(
                        output,
                        create_raw_and_rendered_file(
                            parser,
                            *note_id,
                            ConstructFileDataType::Card(card_order, card, side),
                            &file_contents,
                            overridden_output_raw_dir,
                            render,
                            web_render_mode,
                            Some(&card_image_occlusion_sources),
                            &key,
                            stored(output),
                            reuse,
                        ),
                    )?;
                }
                Ok(())
            })?;
        // Forget the cards the note no longer has
        if render && let Some(states) = render_states {
            let has = |output: &RenderOutputType| match *output {
                RenderOutputType::Note => true,
                RenderOutputType::Card(order, side) => order
                    .checked_sub(1)
                    .and_then(|i| cards.get(i))
                    .is_some_and(|card| sides(card).contains(&side)),
            };
            crate::collection::lock(changes)
                .removed
                .extend(states.keys().filter(|output| !has(output)));
        }
    }

    // Create note files
    record(
        RenderOutputType::Note,
        create_raw_and_rendered_file(
            parser,
            *note_id,
            ConstructFileDataType::Note,
            &file_contents,
            overridden_output_raw_dir,
            render,
            web_render_mode,
            None,
            &note_key,
            None,
            reuse,
        ),
    )
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
    // What the rendered file is made from (see `RenderState::key`)
    key: &str,
    // From when it was last rendered, to render it only if it changed. `None` for notes, which
    // are checked before their cards are made, and when rendering is forced.
    stored: Option<&RenderState>,
    // The batch's cards from before it, to copy a card from instead of rendering it
    reuse: Option<&RenderReuse>,
) -> Result<(PathBuf, Option<RenderState>), Error> {
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

        if stored.is_some_and(|stored| stored.key == key)
            && !raw_changed
            && output_rendered_filepath.exists()
        {
            debug!(
                "{} is up to date. Skipping.",
                output_rendered_filepath.display()
            );
            return Ok((output_text_filepath, None));
        }

        if let Some(reuse) = reuse
            && let ConstructFileDataType::Card(_, card_data, _) = output_type
            && !card_data
                .data
                .iter()
                .any(|part| matches!(part, NotePart::ImageOcclusion { .. }))
            && let Some(dependencies) = reuse.copy(key, &output_rendered_filepath)
        {
            let state = RenderState {
                key: key.to_string(),
                dependencies,
            };
            return Ok((output_text_filepath, Some(state)));
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

        let RenderedFile {
            output,
            mut dependencies,
        } = {
            let render = || {
                parser.render_file(
                    &aux_dir,
                    &output_text_filepath,
                    &output_rendered_dir,
                    &output_rendered_filepath,
                )
            };
            // Others' links to the file being replaced keep what it was (see `RenderReuse`)
            if reuse.is_some() {
                render_aside(&output_rendered_filepath, render)?
            } else {
                render()?
            }
        };

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
                &mut dependencies,
            );
        }

        dependencies.normalize();
        let state = RenderState {
            key: key.to_string(),
            dependencies,
        };
        return Ok((output_text_filepath, Some(state)));
    }

    Ok((output_text_filepath, None))
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

/// Writes the HTML for web clients next to the rendered file, or removes it if they should show
/// the rendered file instead (see [`web_render_filepath`]). Never fails the render, since the
/// rendered file is always there to fall back to.
fn create_web_render_file(
    parser: &dyn Parseable,
    note_id: NoteId,
    output_text_filepath: &Path,
    output_rendered_filepath: &Path,
    mode: WebRenderMode,
    dependencies: &mut RenderDependencies,
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
                dependencies,
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
        match remove_file(&output_web_filepath) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => warn!(
                "[Note Id: {}] Failed to remove {}: {e}",
                note_id.letters(),
                output_web_filepath.display()
            ),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;
    use std::fs::remove_file;
    use std::fs::write;
    use std::path::Path;

    use super::blake3_hex;
    use super::card_render_key;
    use super::create_web_render_file;
    use super::generates_render_files;
    use super::image_occlusion_sources_hash;
    use super::link_into;
    use super::render_key;
    use crate::config::SparesExternalConfig;
    use crate::parsers::RenderDependencies;
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
    fn test_create_web_render_file_removes_stale_html() {
        let dir = std::env::temp_dir().join(format!("spares-web-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let parser = MarkdownParser::new();
        let text = dir.join("0001.md");
        let rendered = dir.join("0001.pdf");
        let html = dir.join("0001.html");
        let tikz = "\\begin{tikzpicture}\n\\end{tikzpicture}\n";
        let render = |contents: &str, mode| {
            write(&text, contents).unwrap();
            create_web_render_file(
                &parser,
                "aaa".parse().unwrap(),
                &text,
                &rendered,
                mode,
                &mut RenderDependencies::default(),
            );
        };

        render("Text", WebRenderMode::Auto);
        assert!(has_web_render_file(&rendered));
        // A later render that decides on the rendered file removes the HTML
        render("Text", WebRenderMode::Pdf);
        assert!(!html.exists());
        assert!(!has_web_render_file(&rendered));
        // As does one with no HTML to remove
        render("Text", WebRenderMode::Pdf);
        assert!(!html.exists());

        render(tikz, WebRenderMode::Html);
        assert!(has_web_render_file(&rendered));
        render(tikz, WebRenderMode::Auto);
        assert!(!has_web_render_file(&rendered));
        assert!(!html.exists());
        assert!(!dir.join("0001.html.tmp").exists());

        // A failed render, here from a missing image, also removes it
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
