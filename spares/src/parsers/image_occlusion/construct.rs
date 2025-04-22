use super::utils::{
    append_to_stem, convert_image_to_png, convert_svg_to_png, get_center_of_shape,
    get_image_occlusion_card_filepath, get_image_occlusion_directory,
    get_image_occlusion_rendered_directory, is_imagemagick_installed,
};
use super::{
    CLOZE_SETTINGS_KEY, CLOZES_GROUP_ID, ConstructImageOcclusionType, FrontConceal,
    ImageOcclusionClozeIndex, ImageOcclusionConfig, ImageOcclusionData, ParsedImageOcclusionCloze,
    SvgClozeType,
};
use crate::config::read_external_config;
use crate::helpers::to_title_case;
use crate::parsers::generate_files::{CardSide, RenderOutputType};
use crate::parsers::{
    BackReveal, CardData, ClozeData, ClozeGroupingSettings, ClozeHiddenReplacement, ClozeSettings,
    ClozeSettingsKeys, NotePart, NoteSettingsKeys, Parseable, parse_card_settings,
};
use crate::{Error, LibraryError, NoteErrorKind};
use std::fs::{self, File, OpenOptions, read_to_string};
use std::ops::Range;
use std::path::Path;
use std::process::Command;
use strum::IntoEnumIterator;
use toml_edit::DocumentMut;
use xmltree::{Element, EmitterConfig};

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

            // SAFETY: The underlying struct is validated to be serializable.
            let image_occlusion_data_toml =
                toml_edit::ser::to_string_pretty(&image_occlusion_data).unwrap();
            // .map_err(|e| {
            //     LibraryError::InvalidConfig(format!(
            //         "Failed to serialize image occlusion data: {}",
            //         e
            //     ))
            // })?;
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
            // parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
            output_rendered_filepath.push(
                parser.get_output_filename(RenderOutputType::Card(card_order, side), note_id),
            );
            let result = construct_image_fn(
                get_image_occlusion_card_filepath(
                    &output_rendered_filepath,
                    side,
                    image_occlusion_order,
                )
                .as_path(),
                &caption,
            );
            result
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
            description: format!("Failed to read {}.", &clozes_filepath.display()),
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
                &clozes_filepath.display()
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
            description: format!("Failed to write file {}.", &clozes_filepath.display()),
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
    let _image_occlusion_card_filepaths = card_data
        .data
        .iter()
        .filter_map(|note_part| match note_part {
            NotePart::ImageOcclusion {
                cloze_indices,
                data,
            } => Some((cloze_indices, data)),
            _ => None,
        })
        .enumerate()
        .map(|(i, (cloze_indices, image_occlusion_data))| {
            let image_occlusion_order_in_card = i + 1;
            let card_filepath = get_image_occlusion_card_filepath(
                image_occlusion_output_rendered_filepath,
                side,
                image_occlusion_order_in_card,
            );
            create_image_occlusion_card(cloze_indices, image_occlusion_data, &card_filepath, side)
        })
        .collect::<Vec<_>>();
    Ok(())
}

pub fn modify_clozes_for_card(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    clozes: &mut [&mut Element],
    front_conceal: FrontConceal,
    back_reveal: BackReveal,
    side: CardSide,
    image_occlusion_config: &ImageOcclusionConfig,
) {
    let ImageOcclusionConfig {
        cloze_to_answer_color,
        cloze_not_to_answer_color,
        cloze_hint_font_size,
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
                            cloze
                                .attributes
                                .insert("fill".to_string(), cloze_to_answer_color.clone());
                            if let Some(hint) = hint {
                                modify_hint_cloze(cloze, hint, *cloze_hint_font_size);
                            }
                        }
                        ClozeHiddenReplacement::NotToAnswer => {
                            modify_not_to_answer_cloze(cloze, cloze_not_to_answer_color);
                        }
                    }
                } else {
                    match front_conceal {
                        FrontConceal::OnlyGrouping => modify_hide_cloze_mask(cloze),
                        FrontConceal::AllGroupings => {
                            modify_not_to_answer_cloze(cloze, cloze_not_to_answer_color);
                        }
                    }
                }
            }
            CardSide::Back => {
                match back_reveal {
                    BackReveal::FullNote => {
                        // Reveal all data by hiding all the cloze masks
                        modify_hide_cloze_mask(cloze);
                    }
                    BackReveal::OnlyAnswered => {
                        if cloze_replacement_opt.is_some() {
                            // Cloze is a part of the grouping, so reveal it by hiding the cloze mask
                            modify_hide_cloze_mask(cloze);
                        }
                        // Otherwise, the cloze is not a part of the grouping and we only want
                        // to reveal the grouping, so keep this cloze mask.
                    }
                }
            }
        }
    }
}

fn modify_hint_cloze(cloze: &mut Element, hint: &str, cloze_hint_font_size: u32) {
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
    // hint_element.attributes.insert("font-family".to_string(), "Verdana".to_string());
    hint_element
        .attributes
        .insert("font-size".to_string(), cloze_hint_font_size.to_string());
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
        .push(xmltree::XMLNode::Text(hint.to_string()));
    cloze.children.push(xmltree::XMLNode::Element(hint_element));
}

fn modify_not_to_answer_cloze(cloze: &mut Element, cloze_not_to_answer_color: &str) {
    cloze
        .attributes
        .insert("fill".to_string(), cloze_not_to_answer_color.to_string());
}

fn modify_hide_cloze_mask(cloze: &mut Element) {
    cloze
        .attributes
        .insert("opacity".to_string(), "0".to_string());
    // .insert("visibility".to_string(), "hidden".to_string());
}

#[allow(clippy::too_many_lines, reason = "off by a few")]
fn create_image_occlusion_card(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    image_occlusion_data: &ImageOcclusionData,
    card_filepath: &Path,
    side: CardSide,
) -> Result<(), Error> {
    let debug = false;
    let ImageOcclusionData {
        original_image_filepath,
        clozes_filepath,
        front_conceal,
        back_reveal,
    } = image_occlusion_data;
    let clozes_file_contents = read_to_string(clozes_filepath).map_err(|_| {
        LibraryError::Note(NoteErrorKind::Other {
            description: format!("Failed to read {}.", &clozes_filepath.display()),
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
            src: clozes_file_contents.to_string(),
            at: (0..clozes_file_contents.len()).into(),
        })
    })?;
    let config = read_external_config()?;
    modify_clozes_for_card(
        cloze_indices,
        &mut clozes,
        *front_conceal,
        *back_reveal,
        side,
        &config.image_occlusion,
    );
    // Write card's clozes to file
    let mut temp_card_cloze_svg_filepath = append_to_stem(card_filepath, "-temp");
    temp_card_cloze_svg_filepath.set_extension("svg");
    // Create file (necessary for xml package)
    File::create(&temp_card_cloze_svg_filepath).map_err(|e| {
        LibraryError::Note(NoteErrorKind::InvalidSettings {
            description: e.to_string(),
            advice: None,
            src: clozes_file_contents.to_string(),
            at: (0..clozes_file_contents.len()).into(),
        })
    })?;
    let _ = clozes_svg_element.write_with_config(
        OpenOptions::new()
            .write(true)
            .open(&temp_card_cloze_svg_filepath)
            .unwrap(),
        EmitterConfig::new().perform_indent(true),
    );

    // Verify imagemagick is installed
    if !is_imagemagick_installed() {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "ImageMagick is not installed".to_string(),
        })));
    }

    // Convert original image to PNG
    let temp_original_image_filepath_png =
        convert_image_to_png(card_filepath, original_image_filepath)?;

    // Convert clozes SVG to PNG with transparent background
    let temp_card_cloze_png_filepath = convert_svg_to_png(&temp_card_cloze_svg_filepath)?;

    // Composite clozes on original image
    let status = Command::new("magick")
        .arg(&temp_original_image_filepath_png)
        .arg(&temp_card_cloze_png_filepath)
        .arg("-gravity")
        .arg("center")
        .arg("-composite")
        .arg(card_filepath)
        .status()
        .map_err(|e| {
            LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Failed to compose clozes and original image to create card file: {}",
                    e
                ),
            })
        })?;
    if !card_filepath.exists() || !status.success() {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "Failed to compose clozes and original image to create card file."
                .to_string(),
        })));
    }

    // Remove temporary files
    if !debug {
        if temp_card_cloze_svg_filepath.exists() {
            fs::remove_file(&temp_card_cloze_svg_filepath).map_err(|e| {
                LibraryError::Note(NoteErrorKind::Other {
                    description: format!("Failed to remove temporary file: {}", e),
                })
            })?;
        }
        if temp_card_cloze_png_filepath.exists() {
            fs::remove_file(&temp_card_cloze_png_filepath).map_err(|e| {
                LibraryError::Note(NoteErrorKind::Other {
                    description: format!("Failed to remove temporary file: {}", e),
                })
            })?;
        }
        if temp_original_image_filepath_png.exists() {
            fs::remove_file(&temp_original_image_filepath_png).map_err(|e| {
                LibraryError::Note(NoteErrorKind::Other {
                    description: format!("Failed to remove temporary file: {}", e),
                })
            })?;
        }
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

#[allow(clippy::too_many_lines, reason = "off by a few")]
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
        .map(|element| {
            if element.name == "g" {
                return Err((
                    "Grouped shapes are not supported.".to_string(),
                    Some("Use the cloze settings to group shapes.".to_string()),
                ));
            }
            Ok(element)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|element| valid_cloze_types.contains(&element.name))
        .collect::<Vec<_>>();
    Ok(clozes)
}

pub fn get_clozes_from_svg_str(
    data: &str,
    front_conceal: FrontConceal,
    back_reveal: BackReveal,
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
    let mut current_grouping_number = 1;
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
                &mut current_grouping_number,
                &note_settings_keys,
                &cloze_settings_keys,
                Some((front_conceal, back_reveal)),
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
