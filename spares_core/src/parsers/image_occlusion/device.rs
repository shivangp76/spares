//! The image occlusions of a device's cards (see [`crate::platform`]).
//!
//! A device renders no cards, so no PNG of a card's image occlusion is ever written on it. And the
//! PNGs the laptop renders are named by the card, so after a change made on the device they show
//! the clozes from before it until the laptop renders the card again. Instead, a card's source
//! names the image by what it's made from ([`card_filepath`]), and the device makes it when the web
//! app asks for it ([`read_card`]): a PNG, or the SVG of the clozes that cards of parsers with
//! [`crate::parsers::Parseable::image_occlusion_overlays`] show over the original image. An image
//! named this way is never out of date, so the web app can keep it as long as it likes. The web app
//! shows it in Typst cards, which it compiles, and in Markdown cards changed on the device, which
//! it shows from their source until the server renders them
//! (`api::browser_render::add_device_sources`).
//!
//! Compiled natively under tests, to test it.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use log::warn;

use super::ImageOcclusionData;
use super::construct::card_clozes_svg;
use super::construct::compose_image_occlusion_card;
use super::construct::image_occlusion_overlay_svg;
use super::get_image_occlusion_rendered_directory;
use crate::Error;
use crate::LibraryError;
use crate::NoteErrorKind;
use crate::config::read_external_config;
use crate::parsers::ClozeHiddenReplacement;
use crate::parsers::generate_files::CardSide;

/// What each image [`card_filepath`] named is made from, by its path
static CARDS: Mutex<BTreeMap<PathBuf, CardImage>> = Mutex::new(BTreeMap::new());

#[derive(Clone)]
struct CardImage {
    original_image_filepath: PathBuf,
    clozes_svg: Vec<u8>,
    /// An overlay instead of a PNG (see [`image_occlusion_overlay_svg`])
    overlay: bool,
}

fn cards() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, CardImage>> {
    // Each operation is one call on the map, so a panic leaves it whole.
    CARDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Where a card's source finds `image_occlusion_data` as `side` of the card shows it, as a PNG or,
/// if `overlay`, an SVG to show over the original image: a path in
/// [`get_image_occlusion_rendered_directory`] named by the hash of the original image's path and
/// the card's clozes SVG. `None` if the clozes can't be read, so the card names the laptop's image
/// instead, and shows like any card whose image is missing.
pub(crate) fn card_filepath(
    cloze_indices: &[(usize, ClozeHiddenReplacement)],
    image_occlusion_data: &ImageOcclusionData,
    side: CardSide,
    overlay: bool,
) -> Option<PathBuf> {
    let made = read_external_config().and_then(|config| {
        card_clozes_svg(
            cloze_indices,
            image_occlusion_data,
            side,
            &config.image_occlusion,
        )
    });
    let clozes_svg = match made {
        Ok(clozes_svg) => clozes_svg,
        Err(e) => {
            warn!(
                "Image occlusion {}: {e}",
                image_occlusion_data.clozes_filepath.display()
            );
            return None;
        }
    };
    let original_image_filepath = image_occlusion_data.original_image_filepath.clone();
    let mut hasher = blake3::Hasher::new();
    hasher.update(original_image_filepath.as_os_str().as_encoded_bytes());
    hasher.update(&[0]);
    hasher.update(&clozes_svg);
    let path = get_image_occlusion_rendered_directory().join(format!(
        "{}.{}",
        hasher.finalize().to_hex(),
        if overlay { "svg" } else { "png" }
    ));
    cards().insert(
        path.clone(),
        CardImage {
            original_image_filepath,
            clozes_svg,
            overlay,
        },
    );
    Some(path)
}

/// The image at `path`, if [`card_filepath`] named it.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
pub async fn read_card(path: &Path) -> Option<Result<Vec<u8>, Error>> {
    let card = cards().get(path).cloned()?;
    let original_image_filepath = &card.original_image_filepath;
    let error =
        |description| Error::Library(LibraryError::Note(NoteErrorKind::Other { description }));
    Some(
        match crate::platform::read(original_image_filepath).await {
            Ok(original_image) if card.overlay => {
                image_occlusion_overlay_svg(&original_image, &card.clozes_svg)
            }
            Ok(original_image) => compose_image_occlusion_card(&original_image, &card.clozes_svg),
            Err(e) => Err(format!("Failed to open input image: {e}")),
        }
        .map_err(|e| error(format!("{e} ({})", original_image_filepath.display()))),
    )
}

#[cfg(test)]
mod tests {
    use image::Rgba;
    use image::RgbaImage;

    use super::*;
    use crate::parsers::BackReveal;
    use crate::parsers::FrontConceal;
    use crate::parsers::image_occlusion::construct::create_image_occlusion_card;
    use crate::parsers::image_occlusion::get_image_occlusion_directory;

    const CLOZES: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
  <g class="layer" id="markup-group"><title>Markup</title></g>
  <g class="layer" id="clozes-group">
    <title>Clozes</title>
    <rect fill="#FFEBA2" height="20" width="30" y="10" id="svg_1" x="10" />
    <rect fill="#FFEBA2" height="20" width="30" y="40" id="svg_2" x="50" />
  </g>
</svg>"##;

    /// The device's image is the laptop's, and its name follows the clozes the card shows.
    #[tokio::test]
    async fn device_card_matches_rendered_card() {
        let dir = get_image_occlusion_directory();
        let original_image_filepath = dir.join("device-card-test.png");
        let mut image = RgbaImage::new(100, 80);
        for pixel in image.pixels_mut() {
            *pixel = Rgba([30, 120, 200, 255]);
        }
        image.save(&original_image_filepath).unwrap();
        let clozes_filepath = dir.join("device-card-test_clozes.svg");
        std::fs::write(&clozes_filepath, CLOZES).unwrap();
        let data = ImageOcclusionData {
            original_image_filepath,
            clozes_filepath,
            front_conceal: FrontConceal::OnlyGrouping,
            back_reveal: BackReveal::FullNote,
            back_emphasis: false,
        };
        let first = [(0, ClozeHiddenReplacement::ToAnswer { hint: None })];
        let second = [(1, ClozeHiddenReplacement::ToAnswer { hint: None })];

        let path = card_filepath(&first, &data, CardSide::Front, false).unwrap();
        assert_eq!(
            path.parent().unwrap(),
            get_image_occlusion_rendered_directory()
        );
        assert_eq!(
            card_filepath(&first, &data, CardSide::Front, false),
            Some(path.clone())
        );
        let others = [
            card_filepath(&second, &data, CardSide::Front, false).unwrap(),
            card_filepath(&first, &data, CardSide::Back, false).unwrap(),
        ];
        assert!(!others.contains(&path));
        assert_ne!(others[0], others[1]);

        let made = read_card(&path).await.unwrap().unwrap();
        let rendered = dir.join("device-card-test-rendered.png");
        let config = read_external_config().unwrap().image_occlusion;
        create_image_occlusion_card(&first, &data, &rendered, CardSide::Front, &config, false)
            .unwrap();
        assert_eq!(made, std::fs::read(&rendered).unwrap());

        // Likewise the overlay, which is named apart from the PNG
        let overlay_path = card_filepath(&first, &data, CardSide::Front, true).unwrap();
        assert_eq!(overlay_path, path.with_extension("svg"));
        let made = read_card(&overlay_path).await.unwrap().unwrap();
        let rendered_overlay = dir.join("device-card-test-rendered.svg");
        create_image_occlusion_card(
            &first,
            &data,
            &rendered_overlay,
            CardSide::Front,
            &config,
            true,
        )
        .unwrap();
        assert_eq!(made, std::fs::read(&rendered_overlay).unwrap());

        assert!(read_card(&dir.join("unknown.png")).await.is_none());
        for path in [
            &data.original_image_filepath,
            &data.clozes_filepath,
            &rendered,
            &rendered_overlay,
        ] {
            std::fs::remove_file(path).unwrap();
        }
    }
}
