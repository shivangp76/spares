use std::fs::create_dir_all;
use std::path::Path;
use std::path::PathBuf;

use image::ImageBuffer;
use image::Rgba;
use image::RgbaImage;
use resvg::tiny_skia::Pixmap;
use resvg::tiny_skia::Transform;
use resvg::usvg::fontdb;
use xmltree::Element;

use super::SvgClozeType;
use crate::config::get_cache_dir;
use crate::config::get_data_dir;
use crate::parsers::generate_files::CardSide;

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
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some(ext) = path.extension().and_then(|e| e.to_str())
    {
        result.set_file_name(format!("{}{}.{}", stem, suffix, ext));
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
        SvgClozeType::Group => {
            // Find the center of the bounding box that encloses all child shapes.
            get_bounding_box_of_group(element).map_or((0.0, 0.0), |(min_x, min_y, max_x, max_y)| {
                (f64::midpoint(min_x, max_x), f64::midpoint(min_y, max_y))
            })
        }
    }
}

/// Returns the axis-aligned bounding box `(min_x, min_y, max_x, max_y)` that
/// covers all recognised primitive-shape children of a `<g>` group element.
/// Returns `None` when the group has no recognised children.
fn get_bounding_box_of_group(group: &Element) -> Option<(f64, f64, f64, f64)> {
    group
        .children
        .iter()
        .filter_map(|node| match node {
            xmltree::XMLNode::Element(el) => el.name.parse::<SvgClozeType>().ok().map(|t| (t, el)),
            _ => None,
        })
        .filter_map(|(shape_type, el)| get_bounding_box_of_shape(shape_type, el))
        .reduce(|(ax, ay, ax2, ay2), (bx, by, bx2, by2)| {
            (ax.min(bx), ay.min(by), ax2.max(bx2), ay2.max(by2))
        })
}

/// Returns `(min_x, min_y, max_x, max_y)` for a single primitive shape.
/// Returns `None` for shapes whose bounds cannot be determined analytically
/// (currently `Path`).
fn get_bounding_box_of_shape(
    shape_type: SvgClozeType,
    element: &Element,
) -> Option<(f64, f64, f64, f64)> {
    let attr = |key: &str| -> f64 {
        element
            .attributes
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or_default()
    };
    match shape_type {
        SvgClozeType::Rectangle => {
            let x = attr("x");
            let y = attr("y");
            let w = attr("width");
            let h = attr("height");
            Some((x, y, x + w, y + h))
        }
        SvgClozeType::Circle => {
            let cx = attr("cx");
            let cy = attr("cy");
            let r = attr("r");
            Some((cx - r, cy - r, cx + r, cy + r))
        }
        SvgClozeType::Ellipse => {
            let cx = attr("cx");
            let cy = attr("cy");
            let rx = attr("rx");
            let ry = attr("ry");
            Some((cx - rx, cy - ry, cx + rx, cy + ry))
        }
        SvgClozeType::Polygon => {
            let points_str = element.attributes.get("points")?;
            let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
            let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for point in points_str.split(' ') {
                let mut parts = point.split(',');
                let x: f64 = parts
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let y: f64 = parts
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            if min_x.is_finite() {
                Some((min_x, min_y, max_x, max_y))
            } else {
                None
            }
        }
        // Path bounds require parsing path data — not attempted here.
        SvgClozeType::Path | SvgClozeType::Group => None,
    }
}

#[allow(clippy::cast_precision_loss)]
#[allow(clippy::cast_sign_loss)]
pub fn render_svg_to_rgba(
    svg_data: &[u8],
    target_w: Option<u32>,
    target_h: Option<u32>,
) -> Result<RgbaImage, String> {
    // let svg_data = std::fs::read(svg_path)
    //     .map_err(|e| format!("Failed to read SVG {}: {}", svg_path.display(), e))?;

    // Prepare usvg options, set base dir for relative resources
    let mut opt = resvg::usvg::Options::default();
    // if let Ok(abs) = std::fs::canonicalize(svg_path) {
    //     if let Some(dir) = abs.parent() {
    //         opt.resources_dir = Some(dir.to_path_buf());
    //     }
    // }

    // Build a font DB and load a bundled fallback font from bytes
    // (compile-time embed; no local file path at runtime).
    let mut db = fontdb::Database::new();
    // Replace with a redistributable font you can ship. Example path:
    db.load_font_data(include_bytes!("./fonts/LibertinusSerif-Regular.ttf").to_vec());
    db.load_font_data(include_bytes!("./fonts/LibertinusSans-Regular.ttf").to_vec());

    // Optional: set generic family fallbacks to your bundled font
    // so 'serif'/'sans-serif' in SVG map to something present.
    db.set_serif_family("Libertinus Serif");
    db.set_sans_serif_family("Libertinus Sans");

    // Use the DB for text layout
    opt.fontdb = db.into();
    opt.font_family = "Libertinus Sans".to_string();

    let tree = resvg::usvg::Tree::from_data(svg_data, &opt)
        .map_err(|e| format!("Failed to parse SVG: {e}"))?;

    // Determine output size based on SVG viewbox and optional target size
    let svg_size = tree.size().to_int_size();
    let (mut out_w, mut out_h) = (svg_size.width(), svg_size.height());

    match (target_w, target_h) {
        (Some(w), Some(h)) => {
            out_w = w.max(1);
            out_h = h.max(1);
        }
        (Some(w), None) => {
            // Preserve aspect ratio
            let aspect = out_h as f32 / out_w as f32;
            out_w = w.max(1);
            out_h = ((out_w as f32) * aspect).round().max(1.0) as u32;
        }
        (None, Some(h)) => {
            let aspect = out_w as f32 / out_h as f32;
            out_h = h.max(1);
            out_w = ((out_h as f32) * aspect).round().max(1.0) as u32;
        }
        (None, None) => {
            // Keep natural size
        }
    }

    let mut pixmap = Pixmap::new(out_w, out_h)
        .ok_or_else(|| format!("Failed to create pixmap of size {out_w}x{out_h}"))?;

    resvg::render(&tree, Transform::default(), &mut pixmap.as_mut());

    // Convert premultiplied RGBA from tiny-skia to unpremultiplied RGBA for image crate
    let unpremul = unpremultiply_rgba(pixmap.data());

    let img = ImageBuffer::<Rgba<u8>, _>::from_raw(out_w, out_h, unpremul)
        .ok_or_else(|| "Failed to build overlay image buffer".to_string())?;

    Ok(img)
}

#[allow(clippy::many_single_char_names)]
pub fn unpremultiply_rgba(premul: &[u8]) -> Vec<u8> {
    debug_assert!(premul.len().is_multiple_of(4));
    let mut out = Vec::with_capacity(premul.len());

    for chunk in premul.chunks_exact(4) {
        let r = u32::from(chunk[0]);
        let g = u32::from(chunk[1]);
        let b = u32::from(chunk[2]);
        let a = u32::from(chunk[3]);

        // .and_then chains the division only if 'a' is not zero.
        // We use a map to apply the rounding formula: (c * 255 + a/2) / a
        let unpremul = a.checked_div(a).map(|_| {
            let r_u = ((r * 255 + (a / 2)) / a).min(255) as u8;
            let g_u = ((g * 255 + (a / 2)) / a).min(255) as u8;
            let b_u = ((b * 255 + (a / 2)) / a).min(255) as u8;
            [r_u, g_u, b_u, a as u8]
        });

        match unpremul {
            Some(pixel) => out.extend_from_slice(&pixel),
            None => out.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: parse a compact XML fragment into an Element.
    fn parse(xml: &str) -> Element {
        Element::parse(xml.as_bytes()).unwrap()
    }

    // ── get_center_of_shape(Group, …) ─────────────────────────────────────────

    /// Two non-overlapping rects → centre of their union bounding box.
    ///   rect1: (0, 0, 100, 40)
    ///   rect2: (200, 10, 300, 30)
    ///   union: (0, 0, 300, 40) → centre (150, 20)
    #[test]
    fn test_group_center_two_rects() {
        let group = parse(
            r#"<g><rect x="0" y="0" width="100" height="40"/><rect x="200" y="10" width="100" height="20"/></g>"#,
        );
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 150.0);
        assert_eq!(cy, 20.0);
    }

    /// A single circle → centre equals (cx, cy) of that circle.
    ///   circle cx=50 cy=30 r=20 → bbox (30, 10, 70, 50) → centre (50, 30)
    #[test]
    fn test_group_center_single_circle() {
        let group = parse(r#"<g><circle cx="50" cy="30" r="20"/></g>"#);
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 50.0);
        assert_eq!(cy, 30.0);
    }

    /// A single ellipse → centre equals (cx, cy) of that ellipse.
    ///   ellipse cx=100 cy=60 rx=40 ry=20 → bbox (60, 40, 140, 80) → centre (100, 60)
    #[test]
    fn test_group_center_single_ellipse() {
        let group = parse(r#"<g><ellipse cx="100" cy="60" rx="40" ry="20"/></g>"#);
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 60.0);
    }

    /// Mixed shapes: rect + ellipse.
    ///   rect   (0,  0, 100, 40)
    ///   ellipse cx=200 cy=20 rx=20 ry=20 → (180, 0, 220, 40)
    ///   union  (0, 0, 220, 40) → centre (110, 20)
    #[test]
    fn test_group_center_rect_and_ellipse() {
        let group = parse(
            r#"<g><rect x="0" y="0" width="100" height="40"/><ellipse cx="200" cy="20" rx="20" ry="20"/></g>"#,
        );
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 110.0);
        assert_eq!(cy, 20.0);
    }

    /// An empty group has no bounding box, so the fallback (0.0, 0.0) is returned.
    #[test]
    fn test_group_center_empty_group() {
        let group = parse(r#"<g/>"#);
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 0.0);
        assert_eq!(cy, 0.0);
    }

    /// A group whose only children are `<path>` elements also returns (0.0, 0.0)
    /// because path bounds are not computed analytically.
    #[test]
    fn test_group_center_only_path_children() {
        let group = parse(r#"<g><path d="M0 0 L100 100"/></g>"#);
        let (cx, cy) = get_center_of_shape(SvgClozeType::Group, &group);
        assert_eq!(cx, 0.0);
        assert_eq!(cy, 0.0);
    }
}
