use super::SvgClozeType;
use crate::{
    Error, LibraryError, NoteErrorKind,
    config::{get_cache_dir, get_data_dir},
    parsers::generate_files::CardSide,
};
use std::{
    fs::{self, create_dir_all},
    path::{Path, PathBuf},
    process::Command,
};
use xmltree::Element;

pub fn get_image_occlusion_directory() -> PathBuf {
    let mut image_occlusions_dir = get_data_dir();
    image_occlusions_dir.push("image_occlusions");
    create_dir_all(&image_occlusions_dir).unwrap();
    image_occlusions_dir
}

pub fn get_image_occlusion_rendered_directory() -> PathBuf {
    let mut image_occlusions_dir = get_cache_dir();
    image_occlusions_dir.push("image_occlusions");
    create_dir_all(&image_occlusions_dir).unwrap();
    image_occlusions_dir
}

pub fn get_image_occlusion_card_filepath(
    output_rendered_filepath: &Path,
    _side: CardSide,
    image_occlusion_order_in_card: usize,
) -> PathBuf {
    // `output_rendered_filepath` is the directory from `get_image_occlusion_renderd_directory()`
    // combined with the card's rendered filename. The card's rendered output directory is not used
    // since this creates more work if the note's parser is changed. Then the image occlusion files
    // would need to be parsed and moved. By using a separate directory, we don't have to move
    // them.
    let mut result = output_rendered_filepath.to_path_buf();
    // NOTE: We don't need to add `front` or `back` in the filename here since
    // `output_rendered_filepath` already contains `front` or `back` depending on the
    // `CardSide`.
    let image_occlusion_stem = format!("-io-{}", image_occlusion_order_in_card);
    result = append_to_stem(&result, &image_occlusion_stem);
    result.set_extension("png");
    result
}

pub fn append_to_stem(path: &Path, suffix: &str) -> PathBuf {
    let mut result = path.to_path_buf();
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            result.set_file_name(format!("{}{}.{}", stem, suffix, ext));
        }
    }
    result
}

#[allow(clippy::cast_precision_loss)]
pub fn get_center_of_shape(shape_type: SvgClozeType, element: &Element) -> (f64, f64) {
    match shape_type {
        SvgClozeType::Rectangle => {
            let x = element
                .attributes
                .get("x")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();
            let y = element
                .attributes
                .get("y")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();
            let height = element
                .attributes
                .get("height")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();
            let width = element
                .attributes
                .get("width")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();

            // Center of rectangle
            let center_x = x + width / 2.0;
            let font_size = 16.;
            // WORKAROUND
            let center_y = y + (height / 2.0) + (font_size / 2.0);
            // let center_y = y + height / 2.0;
            (center_x, center_y)
        }
        SvgClozeType::Ellipse | SvgClozeType::Circle => {
            let cx = element
                .attributes
                .get("cx")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();
            let cy = element
                .attributes
                .get("cy")
                .unwrap()
                .clone()
                .parse::<f64>()
                .unwrap_or_default();
            // Center is cx and cy for ellipse
            (cx, cy)
        }
        SvgClozeType::Polygon => {
            let points = element
                .attributes
                .get("points")
                .unwrap()
                .clone()
                .split(' ')
                .map(|point| {
                    let point_data = point.split(',').collect::<Vec<_>>();
                    assert_eq!(point_data.len(), 2);
                    let x = point_data[0].parse::<f64>().unwrap_or_default();
                    let y = point_data[1].parse::<f64>().unwrap_or_default();
                    (x, y)
                })
                .collect::<Vec<_>>();
            // let points = vec![(50.0, 5.0), (150.0, 5.0), (190.0, 80.0), (10.0, 80.0)];
            // Calculate centroid (average of all points)
            let sum_x: f64 = points.iter().map(|(x, _)| *x).sum();
            let sum_y: f64 = points.iter().map(|(_, y)| *y).sum();
            let center_x = sum_x / points.len() as f64;
            let center_y = sum_y / points.len() as f64;
            (center_x, center_y)
        }
        SvgClozeType::Path => {
            // Default case, return (0, 0) for unknown shapes
            (0.0, 0.0)
        }
    }
}

pub fn convert_image_to_png(
    card_filepath: &Path,
    original_image_filepath: &Path,
) -> Result<PathBuf, Error> {
    let mut temp_original_image_filepath_png = card_filepath.to_path_buf();
    let temp_file_name = format!(
        "{}-temp.png",
        original_image_filepath
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
    );
    temp_original_image_filepath_png.set_file_name(&temp_file_name);
    if original_image_filepath.extension().unwrap() == "png" {
        fs::copy(original_image_filepath, &temp_original_image_filepath_png).map_err(|_| {
            LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Failed to move original image: {}",
                    original_image_filepath.display()
                ),
            })
        })?;
    } else {
        let status = Command::new("magick")
            .arg(original_image_filepath)
            .arg(&temp_original_image_filepath_png)
            .status()
            .map_err(|e| {
                LibraryError::Note(NoteErrorKind::Other {
                    description: format!("Failed to convert original image to png: {}", e),
                })
            })?;
        if !status.success() {
            return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
                description: "Failed to convert original image to png.".to_string(),
            })));
        }
    }
    if !temp_original_image_filepath_png.exists() {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "Failed to get png version of the original image.".to_string(),
        })));
    }
    Ok(temp_original_image_filepath_png)
}

pub fn convert_svg_to_png(temp_card_cloze_svg_filepath: &Path) -> Result<PathBuf, Error> {
    let mut temp_card_cloze_png_filepath = temp_card_cloze_svg_filepath.to_path_buf();
    temp_card_cloze_png_filepath.set_extension("png");
    let status = Command::new("magick")
        // Adding this back causes a problem since the size of the image is different, so the clozes and original image no longer line up when overlaying them.
        // .arg("-density")
        // .arg("1000")
        .arg(temp_card_cloze_svg_filepath)
        .arg("-transparent")
        .arg("white")
        // .arg("-background")
        // .arg("none")
        .arg(&temp_card_cloze_png_filepath)
        .status()
        .map_err(|e| {
            LibraryError::Note(NoteErrorKind::Other {
                description: format!(
                    "Failed to convert clozes svg to png with transparent background: {}",
                    e
                ),
            })
        })?;
    if !temp_card_cloze_png_filepath.exists() || !status.success() {
        return Err(Error::Library(LibraryError::Note(NoteErrorKind::Other {
            description: "Failed to convert clozes svg to png with transparent background."
                .to_string(),
        })));
    }
    Ok(temp_card_cloze_png_filepath)
}

pub fn is_imagemagick_installed() -> bool {
    Command::new("magick")
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
