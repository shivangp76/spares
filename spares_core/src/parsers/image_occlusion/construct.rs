use std::fs::read_to_string;
use std::fs::{self};
use std::ops::Range;
use std::path::Path;

use image::ImageFormat;
use image::imageops;
use rayon::prelude::*;
use strum::IntoEnumIterator;
use toml_edit::DocumentMut;
use xmltree::Element;
use xmltree::EmitterConfig;

use super::CLOZE_SETTINGS_KEY;
use super::CLOZES_GROUP_ID;
use super::ConstructImageOcclusionType;
use super::FrontConceal;
use super::ImageOcclusionClozeIndex;
use super::ImageOcclusionConfig;
use super::ImageOcclusionData;
use super::ParsedImageOcclusionCloze;
use super::SvgClozeType;
use super::utils::append_to_stem;
use super::utils::get_center_of_shape;
use super::utils::get_image_occlusion_card_filepath;
use super::utils::get_image_occlusion_directory;
use super::utils::get_image_occlusion_rendered_directory;
use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::config::read_external_config;
use crate::helpers::to_title_case;
use crate::parsers::BackReveal;
use crate::parsers::CardData;
use crate::parsers::ClozeData;
use crate::parsers::ClozeGroupingSettings;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::ClozeSettings;
use crate::parsers::ClozeSettingsKeys;
use crate::parsers::NotePart;
use crate::parsers::NoteSettingsKeys;
use crate::parsers::Parseable;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::image_occlusion::utils::render_svg_to_rgba;
use crate::parsers::parse_card_settings;

const CLOZE_NOT_TO_ANSWER_TEXT: &str = "(no answer)";

pub fn construct_image_occlusion_from_image(
    parser: &impl Parseable,
    construct_image_fn: fn(file_path: &Path, caption: &str) -> String,
    image_occlusion_data: &ImageOcclusionData,
    output_type: ConstructImageOcclusionType,
) -> String {
    let caption = image_occlusion_data
        .original_image_filepath
        .file_stem()
        .and_then(|x| x.to_str())
        .map_or("Image Occlusion".to_string(), to_title_case);
    match output_type {
        ConstructImageOcclusionType::Note => {
            // Embed image in parser's preferred format so the user can preview it
            let mut result = String::new();
            let start = parser.construct_comment("spares: image occlusion start");
            result.push_str(&start);

            let image_occlusion_data_toml = toml_edit::ser::to_string_pretty(&image_occlusion_data)
                .expect("SAFETY: The underlying struct is validated to be serializable.");
            let image_occlusion_settings_str = image_occlusion_data_toml
                .split('\n')
                .filter(|x| !x.is_empty())
                .map(|x| parser.construct_comment(x))
                .collect::<String>();
            result.push_str(&image_occlusion_settings_str);

            let image_string =
                construct_image_fn(&image_occlusion_data.original_image_filepath, &caption);
            result.push_str(&image_string);
            let end = parser.construct_comment("spares: image occlusion end");
            result.push_str(&end);
            result
        }
        ConstructImageOcclusionType::Card {
            side,
            note_id,
            card_order,
            image_occlusion_order,
        } => {
            let mut output_rendered_filepath = get_image_occlusion_rendered_directory();
            output_rendered_filepath.push(
                parser.get_output_filename(RenderOutputType::Card(card_order, side), note_id),
            );
            construct_image_fn(
                get_image_occlusion_card_filepath(
                    &output_rendered_filepath,
                    side,
                    image_occlusion_order,
                )
                .as_path(),
                &caption,
            )
        }
    }
}

pub fn update_cloze_settings(
    cloze_index: usize, // 0 based index
    cloze_settings_string: &str,
    clozes_filepath: &Path,
    data: &str,
    cloze_range: &Range<usize>,
) -> Result<(), LibraryError> {
    let clozes_file_contents = read_to_string(clozes_filepath).map_err(|_| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to read {}.", clozes_filepath.display()),
            advice: None,
            src: data.to_string(),
            at: cloze_range.clone().into(),
        })
    })?;
    let mut clozes_svg_element = Element::parse(clozes_file_contents.as_bytes()).map_err(|e| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to parse clozes file data as svg: {}", e),
            advice: None,
            src: data.to_string(),
            at: cloze_range.clone().into(),
        })
    })?;
    let mut clozes = get_clozes_from_svg(&mut clozes_svg_element).map_err(|(e, advice)| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: e,
            advice,
            src: data.to_string(),
            at: (0..data.len()).into(),
        })
    })?;
    let relevant_cloze = clozes.get_mut(cloze_index).ok_or_else(|| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!(
                "Failed to find cloze #{} in {}.",
                cloze_index + 1,
                clozes_filepath.display()
            ),
            advice: None,
            src: data.to_string(),
            at: (0..data.len()).into(),
        })
    })?;
    relevant_cloze.attributes.insert(
        CLOZE_SETTINGS_KEY.to_string(),
        cloze_settings_string.to_string(),
    );
    // let _ = clozes_svg_element.write_with_config(
    //     OpenOptions::new()
    //         .write(true)
    //         .open(clozes_filepath)
    //         .unwrap(),
    //     EmitterConfig::new().perform_indent(true),
    // );
    // let clozes_file_contents = read_to_string(clozes_filepath).unwrap();
    // dbg!(&clozes_file_contents);
    // TODO: xmltree bug: Writing directly to the file produces invalid svg data for some reason, but writing to a string first works fine.
    let mut buffer: Vec<u8> = Vec::new();
    let _ = clozes_svg_element
        .write_with_config(&mut buffer, EmitterConfig::new().perform_indent(true));
    let clozes_file_contents = String::from_utf8(buffer).unwrap();
    std::fs::write(clozes_filepath, clozes_file_contents).map_err(|_| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to write file {}.", clozes_filepath.display()),
            advice: None,
            src: data.to_string(),
            at: cloze_range.clone().into(),
        })
    })?;

    Ok(())
}

pub fn create_image_occlusion_cards(
    card_data: &CardData,
    side: CardSide,
    image_occlusion_output_rendered_filepath: &Path,
) -> Result<(), LibraryError> {
    // Collect image occlusion data first
    let image_occlusions = card_data
        .data
        .iter()
        .filter_map(|note_part| match note_part {
            NotePart::ImageOcclusion {
                cloze_indices,
                data,
            } => Some((cloze_indices.as_slice(), data.as_ref())),
            _ => None,
        })
        .enumerate()
        .map(|(i, (cloze_indices, image_occlusion_data))| (i, cloze_indices, image_occlusion_data))
        .collect::<Vec<_>>();

    // Read config once and share across all threads
    let config = read_external_config().map_err(|e| match e {
        Error::Library(le) => le,
        _ => LibraryError::InvalidConfig(format!("Failed to read config: {}", e)),
    })?;

    // Process image occlusions in parallel
    let _image_occlusion_card_filepaths = image_occlusions
        .par_iter()
        .map(|(i, cloze_indices, image_occlusion_data)| {
            let image_occlusion_order_in_card = i + 1;
            let card_filepath = get_image_occlusion_card_filepath(
                image_occlusion_output_rendered_filepath,
                side,
                image_occlusion_order_in_card,
            );
            create_image_occlusion_card(
                cloze_indices,
                image_occlusion_data,
                &card_filepath,
                side,
                &config.image_occlusion,
            )
            .map_err(|e| match e {
                Error::Library(le) => le,
                _ => LibraryError::Note(NoteErrorKind::Other {
                    description: format!(
                        "Failed to create card {}: {}",
                        image_occlusion_order_in_card, e
                    ),
                }),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(())
}

pub fn modify_clozes_for_card(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    clozes: &mut [&mut Element],
    front_conceal: FrontConceal,
    back_reveal: BackReveal,
    back_emphasis: bool,
    side: CardSide,
    image_occlusion_config: &ImageOcclusionConfig,
) {
    let ImageOcclusionConfig {
        cloze_to_answer_color,
        cloze_not_to_answer_color,
        cloze_hint_font_size,
        cloze_emphasis_fill_opacity,
    } = image_occlusion_config;
    // NOTE: We cannot use the original image in any case since there may be markup present in the clozes file that should be shown.
    for (i, cloze) in &mut clozes.iter_mut().enumerate() {
        // Find relevant cloze
        let cloze_replacement_opt = cloze_indices.iter().find(|(j, _)| i == *j);
        match side {
            CardSide::Front => {
                if let Some((_, cloze_replacement)) = cloze_replacement_opt {
                    match cloze_replacement {
                        ClozeHiddenReplacement::ToAnswer { hint } => {
                            set_cloze_color(cloze, cloze_to_answer_color);
                            if let Some(hint) = hint {
                                add_text_to_cloze(cloze, hint, *cloze_hint_font_size);
                            }
                        }
                        ClozeHiddenReplacement::NotToAnswer => {
                            set_cloze_color(cloze, cloze_not_to_answer_color);
                            add_text_to_cloze(
                                cloze,
                                CLOZE_NOT_TO_ANSWER_TEXT,
                                *cloze_hint_font_size,
                            );
                        }
                    }
                } else {
                    match front_conceal {
                        FrontConceal::OnlyGrouping => hide_cloze_mask(cloze),
                        FrontConceal::AllGroupings => {
                            set_cloze_color(cloze, cloze_not_to_answer_color);
                            add_text_to_cloze(
                                cloze,
                                CLOZE_NOT_TO_ANSWER_TEXT,
                                *cloze_hint_font_size,
                            );
                        }
                    }
                }
            }
            CardSide::Back => {
                if let Some((_, r)) = cloze_replacement_opt
                    && matches!(r, ClozeHiddenReplacement::ToAnswer { .. })
                {
                    if back_emphasis {
                        set_cloze_color(cloze, cloze_to_answer_color);
                        set_cloze_fill_opacity(cloze, *cloze_emphasis_fill_opacity);
                    } else {
                        hide_cloze_mask(cloze);
                    }
                } else if matches!(back_reveal, BackReveal::FullNote) {
                    hide_cloze_mask(cloze);
                }
            }
        }
    }
}

fn add_text_to_cloze(cloze: &mut Element, text: &str, text_font_size: u32) {
    let cloze_type_opt: Option<SvgClozeType> = cloze.name.as_str().parse().ok();
    let (center_x, center_y) = cloze_type_opt.map_or((0., 0.), |cloze_type| {
        get_center_of_shape(cloze_type, cloze)
    });
    let current_cloze = cloze.clone();
    cloze.name = "g".to_string();
    cloze.attributes.clear();
    cloze.children.clear();
    cloze
        .children
        .push(xmltree::XMLNode::Element(current_cloze));
    let mut hint_element = cloze.clone();
    hint_element.name = "text".to_string();
    hint_element.children.clear();
    hint_element.attributes.clear();
    hint_element
        .attributes
        .insert("font-size".to_string(), text_font_size.to_string());
    hint_element
        .attributes
        .insert("text-anchor".to_string(), "middle".to_string());
    hint_element
        .attributes
        .insert("dominant-baseline".to_string(), "middle".to_string());
    hint_element
        .attributes
        .insert("x".to_string(), center_x.to_string());
    hint_element
        .attributes
        .insert("y".to_string(), center_y.to_string());
    hint_element
        .children
        .push(xmltree::XMLNode::Text(text.to_string()));
    cloze.children.push(xmltree::XMLNode::Element(hint_element));
}

fn set_cloze_color(cloze: &mut Element, cloze_color: &str) {
    // SVG `fill` on a `<g>` element is inherited by children, but only as a
    // presentation-attribute default — any child that already has an explicit
    // `fill` attribute (e.g. set by the editor) will override it.  We therefore
    // recurse into groups and set the fill directly on every descendant element
    // so the card color is applied uniformly regardless of what the editor wrote.
    if cloze.name == "g" {
        for child in &mut cloze.children {
            if let xmltree::XMLNode::Element(child_element) = child {
                set_cloze_color(child_element, cloze_color);
            }
        }
    } else {
        cloze
            .attributes
            .insert("fill".to_string(), cloze_color.to_string());
    }
}

fn hide_cloze_mask(cloze: &mut Element) {
    cloze
        .attributes
        .insert("opacity".to_string(), "0".to_string());
    // .insert("visibility".to_string(), "hidden".to_string());
}

fn set_cloze_fill_opacity(cloze: &mut Element, cloze_emphasis_fill_opacity: f64) {
    cloze.attributes.insert(
        "fill-opacity".to_string(),
        cloze_emphasis_fill_opacity.to_string(),
    );
}

pub(crate) fn create_image_occlusion_card(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    image_occlusion_data: &ImageOcclusionData,
    card_filepath: &Path,
    side: CardSide,
    config: &ImageOcclusionConfig,
) -> Result<(), Error> {
    let ImageOcclusionData {
        original_image_filepath,
        clozes_filepath,
        front_conceal,
        back_reveal,
        back_emphasis,
    } = image_occlusion_data;
    let clozes_file_contents = read_to_string(clozes_filepath).map_err(|_| {
        LibraryError::Note(NoteErrorKind::Other {
            description: format!("Failed to read {}.", clozes_filepath.display()),
        })
    })?;
    let mut clozes_svg_element = Element::parse(clozes_file_contents.as_bytes()).map_err(|e| {
        LibraryError::Note(NoteErrorKind::Other {
            description: format!("Failed to parse clozes file data as svg: {}", e),
        })
    })?;
    let mut clozes = get_clozes_from_svg(&mut clozes_svg_element).map_err(|(e, advice)| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: e,
            advice,
            src: clozes_file_contents.clone(),
            at: (0..clozes_file_contents.len()).into(),
        })
    })?;
    modify_clozes_for_card(
        cloze_indices,
        &mut clozes,
        *front_conceal,
        *back_reveal,
        *back_emphasis,
        side,
        config,
    );

    // Get card's clozes as bytes
    let mut clozes_file_contents_buffer: Vec<u8> = Vec::new();
    let _ = clozes_svg_element.write_with_config(
        &mut clozes_file_contents_buffer,
        EmitterConfig::new().perform_indent(true),
    );
    // let clozes_file_contents = String::from_utf8(clozes_file_contents_buffer.clone()).unwrap();

    // - x and y: The top-left position (in pixels) where the rendered SVG is placed on the base image. Measured in the base image’s pixel coordinate space. Defaults: 0, 0.
    // - width and height: The output pixel size to render the SVG before compositing.
    //   - Both provided: SVG is scaled to exactly width×height (may distort aspect ratio).
    //   - Only width: Height is computed to preserve the SVG’s aspect ratio.
    //   - Only height: Width is computed to preserve the SVG’s aspect ratio.
    //   - Neither provided: Renders at the SVG’s intrinsic size.
    // - Notes:
    //   - Values are non-negative integers (u32).
    //   - If the overlay extends beyond the base image bounds, it gets clipped.
    let x: u32 = 0;
    let y: u32 = 0;
    let opt_w: Option<u32> = None;
    let opt_h: Option<u32> = None;

    // Load the base image (any supported format: jpeg, png, etc.)
    let mut base_img = image::open(original_image_filepath)
        .map_err(|e| {
            Error::Library(LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Failed to open input image {}: {}",
                    original_image_filepath.display(),
                    e
                ),
            }))
        })?
        .to_rgba8();

    // Render SVG to RGBA (unpremultiplied) with optional scaling
    let overlay_img = render_svg_to_rgba(clozes_file_contents_buffer.as_slice(), opt_w, opt_h)
        .map_err(|e| Error::Library(LibraryError::Note(NoteErrorKind::Other { description: e })))?;

    // Composite overlay onto base at (x, y)
    // imageops::overlay performs alpha blending using unpremultiplied RGBA
    imageops::overlay(&mut base_img, &overlay_img, x.into(), y.into());

    // Save to file as PNG
    base_img
        .save_with_format(card_filepath, ImageFormat::Png)
        .map_err(|e| {
            Error::Library(LibraryError::Note(NoteErrorKind::Other {
                description: format!("Failed to save PNG {}: {}", card_filepath.display(), e),
            }))
        })?;

    if !card_filepath.exists() {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "Failed to compose clozes and original image to create card file."
                .to_string(),
        })));
    }
    Ok(())
}

/// Combines consecutive image occlusion clozes since they all need to be rendered as a part of the same image
fn combine_image_occlusions(
    image_occlusions: &[(ClozeData, ClozeGroupingSettings)],
) -> (ClozeData, ClozeGroupingSettings) {
    let mut result = image_occlusions[0].clone();
    let cloze_indices = image_occlusions
        .iter()
        .enumerate()
        .map(|(i, (cloze_data, grouping_settings))| {
            assert!(&cloze_data.image_occlusion.is_some());
            match &cloze_data.image_occlusion.as_ref().unwrap().index {
                ImageOcclusionClozeIndex::OriginalIndex(index) => {
                    let ClozeSettings { hint, .. } = &image_occlusions.get(i).unwrap().0.settings;
                    let cloze_replacement = if grouping_settings.hidden_no_answer {
                        ClozeHiddenReplacement::NotToAnswer
                    } else {
                        ClozeHiddenReplacement::ToAnswer { hint: hint.clone() }
                    };
                    (*index, cloze_replacement)
                }
                ImageOcclusionClozeIndex::MultipleIndices(_) => unreachable!(),
            }
        })
        .collect::<Vec<_>>();
    result.0.image_occlusion.as_mut().unwrap().index =
        ImageOcclusionClozeIndex::MultipleIndices(cloze_indices);
    result
}

pub fn combine_image_occlusion_clozes(input: &mut Vec<(ClozeData, ClozeGroupingSettings)>) {
    let mut buffer: Vec<(ClozeData, ClozeGroupingSettings)> = Vec::new();
    let mut idx = 0;
    while let Some(item) = input.get_mut(idx) {
        if item.0.image_occlusion.is_some() {
            if let Some(prev_item) = buffer.last() {
                if prev_item.0.start_delim == item.0.start_delim {
                    buffer.push(input.remove(idx));
                    // Skip incrementing `idx` because we removed an item
                } else {
                    let combined = combine_image_occlusions(&buffer);
                    input.insert(idx, combined);
                    buffer.clear();
                    // Remove the next item after inserting combined
                    buffer.push(input.remove(idx + 1));
                    idx += 1;
                }
            } else {
                // Start a new buffer with the first ImageOcclusion
                buffer.push(input.remove(idx));
            }
        } else {
            if !buffer.is_empty() {
                let combined = combine_image_occlusions(&buffer);
                input.insert(idx, combined);
                idx += 1;
                buffer.clear();
            }
            idx += 1;
        }
    }

    // Combine any remaining buffered ImageOcclusion variants
    if !buffer.is_empty() {
        input.push(combine_image_occlusions(&buffer));
    }

    assert!(
        input
            .iter()
            .filter_map(|(data, _)| data.image_occlusion.as_ref())
            .all(|image_occlusion| matches!(
                image_occlusion.index,
                ImageOcclusionClozeIndex::MultipleIndices(_)
            ))
    );
}

#[expect(clippy::too_many_lines)]
pub fn read_image_occlusion_data(
    data: &str,
    setting_capture_range: &[Range<usize>],
    image_occlusion_capture_range: Range<usize>,
    move_files: bool,
) -> Result<ImageOcclusionData, LibraryError> {
    let settings_str = setting_capture_range
        .iter()
        .map(|r| &data[r.start..r.end])
        .collect::<Vec<_>>()
        .join("\n");
    let doc = settings_str.parse::<DocumentMut>().map_err(|e| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!("Failed to parse image occlusion data: {}", e),
            advice: None,
            src: data.to_string(),
            at: image_occlusion_capture_range.clone().into(),
        })
    })?;
    let mut image_occlusion_data: ImageOcclusionData =
        toml_edit::de::from_document(doc).map_err(|e| {
            LibraryError::Note(NoteErrorKind::InvalidSettings {
                description: format!("Failed to parse image occlusion data: {}", e),
                advice: None,
                src: data.to_string(),
                at: image_occlusion_capture_range.clone().into(),
            })
        })?;
    if !image_occlusion_data.original_image_filepath.exists() {
        return Err(LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!(
                "Failed to find image occlusion image: {}",
                image_occlusion_data.original_image_filepath.display()
            ),
            advice: None,
            src: data.to_string(),
            at: image_occlusion_capture_range.clone().into(),
        }));
    }
    if !image_occlusion_data.clozes_filepath.exists() {
        return Err(LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: format!(
                "Failed to find image occlusion clozes image: {}",
                image_occlusion_data.clozes_filepath.display()
            ),
            advice: None,
            src: data.to_string(),
            at: image_occlusion_capture_range.clone().into(),
        }));
    }
    // Move image files
    // Cases to check: 2 different image occlusions that have:
    // - different original image file and different cloze file: Main case
    // - different original image file and same cloze file
    // - same original image file and different cloze file
    // - same original image file and same cloze file
    //
    // Original Image
    let original_image_filename = image_occlusion_data
        .original_image_filepath
        .file_name()
        .ok_or(LibraryError::Note(NoteErrorKind::Other {
            description: format!(
                "Failed to get file name: {}",
                image_occlusion_data.original_image_filepath.display()
            ),
        }))?;
    let new_image_filepath = get_image_occlusion_directory().join(original_image_filename);
    let move_original_image_file =
        image_occlusion_data.original_image_filepath != new_image_filepath;
    if move_original_image_file {
        if new_image_filepath.exists() {
            return Err(LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "An image occlusion file with the same name already exists. Please rename the file.: {}",
                    image_occlusion_data.clozes_filepath.display()
                ),
            }));
        }
        if move_files {
            fs::rename(
                &image_occlusion_data.original_image_filepath,
                &new_image_filepath,
            )
            .map_err(|_| {
                LibraryError::Note(NoteErrorKind::Other {
                    description: format!(
                        "Failed to move image occlusion image: {}",
                        image_occlusion_data.original_image_filepath.display()
                    ),
                })
            })?;
            image_occlusion_data
                .original_image_filepath
                .clone_from(&new_image_filepath);
        }
    }

    // Clozes
    let mut new_cloze_filepath = append_to_stem(&new_image_filepath, "_clozes");
    new_cloze_filepath.set_extension("svg");
    let existing_cloze_file = image_occlusion_data.clozes_filepath == new_cloze_filepath;
    if !existing_cloze_file {
        if new_cloze_filepath.exists() {
            return Err(LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Failed to move image occlusion cloze image since file already exists: {}",
                    image_occlusion_data.clozes_filepath.display()
                ),
            }));
        }
        if move_files {
            fs::rename(&image_occlusion_data.clozes_filepath, &new_cloze_filepath).map_err(
                |_| {
                    LibraryError::Note(NoteErrorKind::Other {
                        description: format!(
                            "Failed to move image occlusion cloze image: {}",
                            image_occlusion_data.clozes_filepath.display()
                        ),
                    })
                },
            )?;
            image_occlusion_data.clozes_filepath = new_cloze_filepath;
        }
    }

    Ok(image_occlusion_data)
}

pub fn get_clozes_from_svg(
    svg_element: &mut Element,
) -> Result<Vec<&mut Element>, (String, Option<String>)> {
    let clozes_group = svg_element
        .children
        .iter_mut()
        .find(|child| match child {
            xmltree::XMLNode::Element(element) => {
                element.name == "g"
                    && element
                        .attributes
                        .get("id")
                        .is_some_and(|id| id == CLOZES_GROUP_ID)
            }
            _ => false,
        })
        .map(|x| match x {
            xmltree::XMLNode::Element(element) => element,
            _ => unreachable!(),
        })
        .ok_or((
            format!("Failed to get '{}' in image occlusion", CLOZES_GROUP_ID),
            None,
        ))?;
    // <https://developer.mozilla.org/en-US/docs/Web/SVG/Tutorial/Basic_Shapes>
    let valid_cloze_types = SvgClozeType::iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>();
    let clozes = clozes_group
        .children
        .iter_mut()
        .filter_map(|child| match child {
            xmltree::XMLNode::Element(element) => Some(element),
            _ => None,
        })
        .filter(|element| valid_cloze_types.contains(&element.name))
        .collect::<Vec<_>>();
    Ok(clozes)
}

pub fn get_clozes_from_svg_str(
    data: &str,
    front_conceal: FrontConceal,
    back_reveal: BackReveal,
    back_emphasis: bool,
    current_grouping_number: &mut u32,
) -> Result<Vec<ParsedImageOcclusionCloze>, LibraryError> {
    let mut svg_element = Element::parse(data.as_bytes()).map_err(|e| {
        LibraryError::Note(NoteErrorKind::Other {
            description: format!("Failed to parse clozes file data as svg: {}", e),
        })
    })?;
    let clozes = get_clozes_from_svg(&mut svg_element).map_err(|(e, advice)| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: e,
            advice,
            src: data.to_string(),
            at: (0..data.len()).into(),
        })
    })?;
    let note_settings_keys = NoteSettingsKeys::default();
    let cloze_settings_keys = ClozeSettingsKeys::default();
    let result = clozes
        .into_iter()
        .map(|element| {
            element
                .attributes
                .get(CLOZE_SETTINGS_KEY)
                .cloned()
                .unwrap_or_default()
        })
        .map(|cloze_settings_string| {
            parse_card_settings(
                &cloze_settings_string,
                &(0..cloze_settings_string.len()),
                current_grouping_number,
                &note_settings_keys,
                &cloze_settings_keys,
                Some((front_conceal, back_reveal, back_emphasis)),
            )
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(settings, grouping_settings)| ParsedImageOcclusionCloze {
            settings,
            grouping_settings,
        })
        .collect::<Vec<_>>();
    Ok(result)
}
