//! An image occlusion without its masks, which the review page shows on a card's back: the
//! original image, with the Markup layer of its clozes SVG over it when that layer has shapes.
//!
//! Like a Typst card's image occlusion, it is two images shown at the same width, never one PNG:
//! the original image as it is, and [`unmasked_overlay_svg`]. So nothing is decoded or encoded,
//! and nothing is made until the review page asks for it.

use std::collections::HashSet;
use std::path::Path;

use xmltree::Element;
use xmltree::EmitterConfig;
use xmltree::XMLNode;

use super::CLOZES_GROUP_ID;
use super::ImageOcclusionData;
use super::construct::get_clozes_from_svg;
use super::construct::overlay_svg_of_size;
use super::construct::read_clozes_svg;
use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::parsers::BackReveal;
use crate::parsers::ClozeHiddenReplacement;

/// Whether the back of a card shows a mask of `image_occlusion_data`, whose clozes at
/// `cloze_indices` the card asks for, as [`super::construct::modify_clozes_for_card`] draws the
/// back: the clozes it asks for are hidden unless they are emphasized, and the others unless the
/// back reveals the full note.
///
/// `cloze_count` is how many clozes the image has, called only when the answer depends on it: a
/// back that reveals only the answered clozes, without emphasis, and asks for no cloze not to be
/// answered. If it is `None` (the clozes SVG couldn't be read), the back is taken to mask some.
pub fn masks_on_back(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    image_occlusion_data: &ImageOcclusionData,
    cloze_count: impl FnOnce() -> Option<usize>,
) -> bool {
    let answered: HashSet<usize> = cloze_indices
        .iter()
        .filter(|(_, r)| matches!(r, ClozeHiddenReplacement::ToAnswer { .. }))
        .map(|(i, _)| *i)
        .collect();
    if image_occlusion_data.back_emphasis && !answered.is_empty() {
        return true;
    }
    match image_occlusion_data.back_reveal {
        BackReveal::FullNote => false,
        BackReveal::OnlyAnswered => {
            cloze_indices
                .iter()
                .any(|(_, r)| matches!(r, ClozeHiddenReplacement::NotToAnswer))
                || cloze_count().is_none_or(|count| (0..count).any(|i| !answered.contains(&i)))
        }
    }
}

/// How many clozes the clozes SVG at `clozes_filepath` has, or `None` if it can't be read.
pub fn cloze_count(clozes_filepath: &Path) -> Option<usize> {
    let mut element = read_clozes_svg(clozes_filepath).ok()?.element;
    get_clozes_from_svg(&mut element).ok().map(|c| c.len())
}

/// The Markup layer of `image_occlusion_data`'s clozes SVG, as an image the size of
/// `original_image`, its original image's contents, to show over it at the same width (see
/// [`overlay_svg_of_size`]): the clozes SVG with its Clozes layer emptied. `None` if it has no
/// shapes once the clozes are gone, as most image occlusions have no markup.
///
/// Of `original_image` only the header is read, for its size.
pub fn unmasked_overlay_svg(
    image_occlusion_data: &ImageOcclusionData,
    original_image: &[u8],
) -> Result<Option<Vec<u8>>, Error> {
    let clozes_filepath = &image_occlusion_data.clozes_filepath;
    let mut svg = read_clozes_svg(clozes_filepath)?.element;
    let error = |description: String| {
        Error::Library(LibraryError::Note(NoteErrorKind::Other { description }))
    };
    let clozes_group = svg
        .children
        .iter_mut()
        .find_map(|child| match child {
            XMLNode::Element(e)
                if e.name == "g"
                    && e.attributes
                        .get("id")
                        .is_some_and(|id| id == CLOZES_GROUP_ID) =>
            {
                Some(e)
            }
            _ => None,
        })
        .ok_or_else(|| {
            error(format!(
                "Failed to get '{CLOZES_GROUP_ID}' in {}",
                clozes_filepath.display()
            ))
        })?;
    // Its title names the layer, as in the editor
    clozes_group
        .children
        .retain(|child| matches!(child, XMLNode::Element(e) if e.name == "title"));
    if !has_shapes(&svg) {
        return Ok(None);
    }
    let size = image_size(original_image).map_err(|e| {
        error(format!(
            "Failed to open input image {}: {e}",
            image_occlusion_data.original_image_filepath.display()
        ))
    })?;
    let mut markup = Vec::new();
    svg.write_with_config(&mut markup, EmitterConfig::new().perform_indent(true))
        .map_err(|e| error(format!("Failed to write the markup SVG: {e}")))?;
    Ok(Some(overlay_svg_of_size(&markup, size)))
}

/// Whether `element` draws anything: has an element that isn't a group or says what the drawing
/// is, such as a `<title>`.
fn has_shapes(element: &Element) -> bool {
    const NOT_DRAWN: [&str; 6] = ["g", "title", "desc", "metadata", "defs", "style"];
    element.children.iter().any(|child| match child {
        XMLNode::Element(e) if e.name == "g" => has_shapes(e),
        XMLNode::Element(e) => !NOT_DRAWN.contains(&e.name.as_str()),
        _ => false,
    })
}

/// The width and height of the image whose contents are `image`, in any format the `image`
/// crate reads, from its header.
fn image_size(image: &[u8]) -> Result<(u32, u32), String> {
    image::ImageReader::new(std::io::Cursor::new(image))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::FrontConceal;
    use crate::parsers::image_occlusion::get_image_occlusion_directory;

    const MARKUP: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
  <g class="layer" id="markup-group"><title>Markup</title><circle cx="50" cy="40" r="5" fill="red" id="svg_9" /></g>
  <g class="layer" id="clozes-group">
    <title>Clozes</title>
    <rect fill="#FFEBA2" height="20" width="30" y="10" id="svg_1" x="10" />
    <rect fill="#FFEBA2" height="20" width="30" y="40" id="svg_2" x="50" />
  </g>
</svg>"##;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    fn data(name: &str, clozes: &str) -> ImageOcclusionData {
        let dir = get_image_occlusion_directory();
        std::fs::create_dir_all(&dir).unwrap();
        let clozes_filepath = dir.join(format!("{name}_clozes.svg"));
        std::fs::write(&clozes_filepath, clozes).unwrap();
        ImageOcclusionData {
            original_image_filepath: dir.join(format!("{name}.png")),
            clozes_filepath,
            front_conceal: FrontConceal::AllGroupings,
            back_reveal: BackReveal::OnlyAnswered,
            back_emphasis: true,
        }
    }

    #[test]
    fn overlay_keeps_markup_and_drops_clozes() {
        let data = data("unmask-markup", MARKUP);
        let svg = unmasked_overlay_svg(&data, &png(200, 160))
            .unwrap()
            .unwrap();
        let svg = String::from_utf8(svg).unwrap();
        // The original image's size, not the clozes SVG's
        assert!(
            svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="160" viewBox="0 0 200 160">"#),
            "{svg}"
        );
        assert!(svg.contains("<circle"), "{svg}");
        assert!(!svg.contains("<rect"), "{svg}");
        assert!(svg.contains("<title>Clozes</title>"), "{svg}");
        // One document, so a browser shows it
        assert!(Element::parse(svg.as_bytes()).is_ok(), "{svg}");
    }

    #[test]
    fn overlay_is_none_without_markup() {
        let without = MARKUP.replace(
            r#"<circle cx="50" cy="40" r="5" fill="red" id="svg_9" />"#,
            "",
        );
        let data = data("unmask-no-markup", &without);
        assert_eq!(unmasked_overlay_svg(&data, &png(100, 80)).unwrap(), None);
    }

    #[test]
    fn masks_on_back_follows_the_back() {
        let data = data("unmask-masks", MARKUP);
        let first = [(0, ClozeHiddenReplacement::ToAnswer { hint: None })];
        let both = [
            (0, ClozeHiddenReplacement::ToAnswer { hint: None }),
            (1, ClozeHiddenReplacement::ToAnswer { hint: None }),
        ];
        let not_to_answer = [
            (0, ClozeHiddenReplacement::ToAnswer { hint: None }),
            (1, ClozeHiddenReplacement::NotToAnswer),
        ];
        let count = || cloze_count(&data.clozes_filepath);
        assert_eq!(count(), Some(2));
        let with = |back_reveal, back_emphasis| ImageOcclusionData {
            back_reveal,
            back_emphasis,
            ..data.clone()
        };
        let never = || -> Option<usize> { panic!("the clozes SVG shouldn't be read") };

        // Emphasized answers are still masks
        assert!(masks_on_back(
            &both,
            &with(BackReveal::FullNote, true),
            never
        ));
        assert!(!masks_on_back(
            &both,
            &with(BackReveal::FullNote, false),
            never
        ));
        assert!(!masks_on_back(
            &not_to_answer,
            &with(BackReveal::FullNote, false),
            never
        ));
        // The clozes not answered stay masked
        assert!(masks_on_back(
            &first,
            &with(BackReveal::OnlyAnswered, false),
            count
        ));
        assert!(masks_on_back(
            &not_to_answer,
            &with(BackReveal::OnlyAnswered, false),
            never
        ));
        // Unless the card answers all of them
        assert!(!masks_on_back(
            &both,
            &with(BackReveal::OnlyAnswered, false),
            count
        ));
        assert!(masks_on_back(
            &both,
            &with(BackReveal::OnlyAnswered, false),
            || None
        ));
    }
}
