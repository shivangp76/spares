//! The text drawn on a hidden cloze of a card: its hint, or "(no answer)". It is sized to fill
//! the cloze's shape, and wrapped onto more lines when that makes it larger (see
//! [`layout_hint`]), since a fixed font size is unreadable on large images and overflows small
//! clozes.
//!
//! Text is measured with the fonts PNG cards are rendered with ([`FONT_DB`]). Each line is given
//! its measured width as `textLength`, so renderers with other fonts, like Typst for the SVG
//! overlays, draw it the same size.

use std::sync::Arc;
use std::sync::LazyLock;

use resvg::tiny_skia::Pixmap;
use resvg::tiny_skia::Transform;
use resvg::usvg;
use resvg::usvg::fontdb;
use xmltree::Element;
use xmltree::EmitterConfig;
use xmltree::XMLNode;

use super::ImageOcclusionConfig;
use super::SvgClozeType;
use super::utils::get_bounding_box_of_shape;

/// The family of hints. Bundled in [`FONT_DB`], and built into Typst, which draws the SVG overlays
/// of Typst cards, so hints look the same on every card.
pub(crate) const HINT_FONT_FAMILY: &str = "Libertinus Serif";
/// The family of other text in clozes files, which draws markup made in the editor.
const DEFAULT_FONT_FAMILY: &str = "Libertinus Sans";
/// Most lines a hint is wrapped onto.
const MAX_LINES: usize = 8;
/// How much larger wrapping onto another line must make a hint, so that a hint which fits about
/// as well on fewer lines stays on them.
const WRAP_GAIN: f64 = 1.1;
/// Text is measured at this size, then scaled, since widths are proportional to the size.
const MEASURE_FONT_SIZE: f64 = 100.;

/// The fonts that SVGs of image occlusions are rendered with, and that hints are measured with.
pub(crate) static FONT_DB: LazyLock<Arc<fontdb::Database>> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    // Compile-time embed, so no font files are needed at runtime
    db.load_font_data(include_bytes!("./fonts/LibertinusSerif-Regular.ttf").to_vec());
    db.load_font_data(include_bytes!("./fonts/LibertinusSans-Regular.ttf").to_vec());
    // So that `serif` and `sans-serif` in an SVG map to a bundled font
    db.set_serif_family("Libertinus Serif");
    db.set_sans_serif_family(DEFAULT_FONT_FAMILY);
    Arc::new(db)
});

/// Options for parsing SVGs with the fonts of [`FONT_DB`].
pub(crate) fn usvg_options() -> usvg::Options<'static> {
    usvg::Options {
        fontdb: FONT_DB.clone(),
        font_family: DEFAULT_FONT_FAMILY.to_string(),
        ..Default::default()
    }
}

/// An axis-aligned rectangle in the coordinates of the clozes group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2., self.y + self.height / 2.)
    }
}

/// How large hints may be, from `[image_occlusion]` in the config and the size of the image.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HintLimits {
    /// Fraction of the text area's width and height a hint may fill.
    pub fill: f64,
    pub min_font_size: f64,
    pub max_font_size: f64,
    /// The image, which hints are kept inside.
    pub image: Option<Bounds>,
}

impl HintLimits {
    /// `image_size` is the width and height of the image, if known. Without it, hints are only
    /// limited by their clozes and `cloze_hint_font_size`.
    pub fn new(config: &ImageOcclusionConfig, image_size: Option<(f64, f64)>) -> Self {
        let longest_side = image_size.map(|(w, h)| w.max(h)).filter(|s| *s > 0.);
        let mut max_font_size =
            longest_side.map_or(f64::INFINITY, |s| s * config.cloze_hint_max_font_fraction);
        if let Some(size) = config.cloze_hint_font_size {
            max_font_size = max_font_size.min(f64::from(size));
        }
        let min_font_size = longest_side
            .map_or(0., |s| s * config.cloze_hint_min_font_fraction)
            .min(max_font_size);
        Self {
            fill: config.cloze_hint_fill.clamp(0.05, 1.),
            min_font_size,
            max_font_size,
            image: image_size.map(|(width, height)| Bounds {
                x: 0.,
                y: 0.,
                width,
                height,
            }),
        }
    }
}

/// A hint broken into lines at a font size.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HintLayout {
    pub font_size: f64,
    /// Each line and its width at `font_size`.
    pub lines: Vec<(String, f64)>,
    pub center: (f64, f64),
}

/// The bounds of `cloze` in the coordinates it is drawn in, i.e. with its own `transform` but
/// not those of its parents. Computed by usvg, so paths, transforms and groups are handled.
/// Falls back to the attributes of basic shapes when usvg cannot compute them.
pub(crate) fn cloze_bounds(cloze: &Element) -> Option<Bounds> {
    usvg_bounds(cloze).or_else(|| {
        let shape_type: SvgClozeType = cloze.name.parse().ok()?;
        let (min_x, min_y, max_x, max_y) = get_bounding_box_of_shape(shape_type, cloze)?;
        Some(Bounds {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        })
    })
}

fn usvg_bounds(cloze: &Element) -> Option<Bounds> {
    let tree = drawable_tree(cloze)?;
    let rect = tree.root().abs_bounding_box();
    (rect.width() > 0. && rect.height() > 0.).then(|| Bounds {
        x: f64::from(rect.x()),
        y: f64::from(rect.y()),
        width: f64::from(rect.width()),
        height: f64::from(rect.height()),
    })
}

/// `cloze` alone, filled black without a stroke, as usvg parses it.
fn drawable_tree(cloze: &Element) -> Option<usvg::Tree> {
    let mut cloze = cloze.clone();
    // usvg drops what is not drawn, which a cloze hidden or without paint would be
    make_drawable(&mut cloze);
    let mut buffer = Vec::new();
    cloze
        .write_with_config(
            &mut buffer,
            EmitterConfig::new().write_document_declaration(false),
        )
        .ok()?;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1" height="1">{}</svg>"#,
        String::from_utf8(buffer).ok()?
    );
    usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()
}

fn make_drawable(element: &mut Element) {
    for key in ["opacity", "display", "visibility", "fill-opacity", "style"] {
        element.attributes.shift_remove(key);
    }
    element
        .attributes
        .insert("fill".to_string(), "#000".to_string());
    // Text should fit inside the stroke, not half way over it
    element
        .attributes
        .insert("stroke".to_string(), "none".to_string());
    for child in &mut element.children {
        if let XMLNode::Element(child) = child {
            make_drawable(child);
        }
    }
}

/// Resolution of the mask that [`text_areas`] finds rectangles in, along the longer side.
const MASK_RESOLUTION: f64 = 96.;

/// Rectangles inside `cloze` (and inside `image`, if given) that text can be placed in: every
/// maximal rectangle of a mask of the cloze, so whatever the proportions of the text, the
/// largest place for it is among them. `bounds` are the cloze's, from [`cloze_bounds`].
///
/// A rect without a `transform` is its own only rectangle. Other shapes are drawn into the mask:
/// an ellipse, a rotated rect, a triangle or a freehand path has room only well inside its
/// bounding box. Falls back to `bounds` if nothing is found.
pub(crate) fn text_areas(cloze: &Element, bounds: Bounds, image: Option<Bounds>) -> Vec<Bounds> {
    // Text past the edge of the image would be cut off
    let bounds_in_image = image.map_or(Some(bounds), |image| bounds.intersection(image));
    let Some(region) = bounds_in_image else {
        return vec![bounds];
    };
    if cloze.name == "rect" && !cloze.attributes.contains_key("transform") {
        return vec![region];
    }
    match mask_rectangles(cloze, region) {
        Some(areas) if !areas.is_empty() => areas,
        _ => vec![region],
    }
}

impl Bounds {
    fn intersection(&self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > x && bottom > y).then_some(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

/// The maximal rectangles of the cells of `region` that `cloze` covers completely.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn mask_rectangles(cloze: &Element, region: Bounds) -> Option<Vec<Bounds>> {
    let tree = drawable_tree(cloze)?;
    let scale = MASK_RESOLUTION / region.width.max(region.height);
    let columns = (region.width * scale).ceil().max(1.) as usize;
    let rows = (region.height * scale).ceil().max(1.) as usize;
    let mut pixmap = Pixmap::new(columns as u32, rows as u32)?;
    let transform = Transform::from_row(
        scale as f32,
        0.,
        0.,
        scale as f32,
        (-region.x * scale) as f32,
        (-region.y * scale) as f32,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // Anti-aliased edges are partly covered, so outside
    let inside = |row: usize, column: usize| pixmap.pixels()[row * columns + column].alpha() == 255;

    // Each row's histogram of how many cells up are inside, and with a stack the widest run of
    // bars at least as tall as each bar: together, every maximal rectangle.
    let mut heights = vec![0usize; columns];
    let mut areas = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for row in 0..rows {
        for (column, height) in heights.iter_mut().enumerate() {
            *height = if inside(row, column) { *height + 1 } else { 0 };
        }
        stack.clear();
        for column in 0..=columns {
            let height = heights.get(column).copied().unwrap_or(0);
            while let Some(&top) = stack.last() {
                if heights[top] < height {
                    break;
                }
                stack.pop();
                let bar = heights[top];
                let left = stack.last().map_or(0, |&l| l + 1);
                if bar > 0 && (heights[top] > height || column == columns) {
                    areas.push(Bounds {
                        x: region.x + left as f64 / scale,
                        y: region.y + (row + 1 - bar) as f64 / scale,
                        width: (column - left) as f64 / scale,
                        height: bar as f64 / scale,
                    });
                }
            }
            stack.push(column);
        }
    }
    Some(areas)
}

/// Widths of text and spaces, and the height of a line, per unit of font size.
struct Metrics {
    space_width: f64,
    line_height: f64,
}

static METRICS: LazyLock<Metrics> = LazyLock::new(|| {
    let with_space = measure("x x");
    let without = measure("xx");
    Metrics {
        space_width: with_space.map_or(0.25, |(w, _)| w) - without.map_or(0., |(w, _)| w),
        line_height: without.map_or(1.15, |(_, h)| h),
    }
});

/// The width and height of a line of `text` per unit of font size: its advances and line box.
fn measure(text: &str) -> Option<(f64, f64)> {
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><text font-family="{HINT_FONT_FAMILY}" font-size="{MEASURE_FONT_SIZE}">{}</text></svg>"#,
        escape_xml(text)
    );
    let tree = usvg::Tree::from_str(&svg, &usvg_options()).ok()?;
    let rect = tree.root().abs_bounding_box();
    Some((
        f64::from(rect.width()) / MEASURE_FONT_SIZE,
        f64::from(rect.height()) / MEASURE_FONT_SIZE,
    ))
}

fn text_width(text: &str) -> f64 {
    if text.is_empty() {
        return 0.;
    }
    measure(text).map_or_else(
        // Roughly the average advance of Libertinus
        #[allow(clippy::cast_precision_loss)]
        || text.chars().count() as f64 * 0.5,
        |(w, _)| w,
    )
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Lays out `text` in the best of `areas`, from [`text_areas`]: the largest font size at which it
/// fits in `limits.fill` of an area on some number of lines, broken between words so the longest
/// line is as short as possible. The size is then kept within the limits' minimum and maximum,
/// so a hint on a tiny cloze stays readable even though it overflows the cloze. `None` if `text`
/// is blank or there are no areas.
pub(crate) fn layout_hint(text: &str, areas: &[Bounds], limits: &HintLimits) -> Option<HintLayout> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let metrics = &*METRICS;
    let word_widths: Vec<f64> = words.iter().map(|w| text_width(w)).collect();
    #[allow(clippy::cast_precision_loss)]
    let size_for = |area: &Bounds, lines: usize, widest: f64| {
        (area.width * limits.fill / widest.max(f64::EPSILON))
            .min(area.height * limits.fill / (lines as f64 * metrics.line_height))
    };
    // The area where text of `lines` lines `widest` wide is largest, and that size
    let best_area = |lines: usize, widest: f64| {
        areas
            .iter()
            .map(|area| (size_for(area, lines, widest), area))
            .max_by(|(a, _), (b, _)| a.total_cmp(b))
    };

    let mut best: Option<(f64, Vec<usize>)> = None;
    for lines in 1..=words.len().min(MAX_LINES) {
        let (widest, breaks) = balanced_breaks(&word_widths, metrics.space_width, lines);
        let (size, _) = best_area(lines, widest)?;
        if best
            .as_ref()
            .is_none_or(|(best_size, _)| size > best_size * WRAP_GAIN)
        {
            best = Some((size, breaks));
        }
    }
    let (_, breaks) = best?;

    // Measure the chosen lines as a whole, which includes kerning between words
    let lines: Vec<String> = breaks
        .windows(2)
        .map(|w| words[w[0]..w[1]].join(" "))
        .collect();
    let widths: Vec<f64> = lines.iter().map(|l| text_width(l)).collect();
    let widest = widths.iter().copied().fold(0., f64::max);
    let (size, area) = best_area(lines.len(), widest)?;
    let font_size = size.min(limits.max_font_size).max(limits.min_font_size);
    Some(HintLayout {
        font_size,
        lines: lines
            .into_iter()
            .zip(widths)
            .map(|(line, width)| (line, width * font_size))
            .collect(),
        center: area.center(),
    })
}

/// Breaks words of `widths` into `lines` lines so the widest is as narrow as possible. Returns
/// that width and the indices of the words starting each line, followed by the number of words.
fn balanced_breaks(widths: &[f64], space_width: f64, lines: usize) -> (f64, Vec<usize>) {
    let n = widths.len();
    let mut prefix = vec![0.; n + 1];
    for (i, w) in widths.iter().enumerate() {
        prefix[i + 1] = prefix[i] + w;
    }
    #[allow(clippy::cast_precision_loss)]
    let line_width = |a: usize, b: usize| prefix[b] - prefix[a] + (b - a - 1) as f64 * space_width;
    // `widest[k][i]`: the narrowest widest line of the first `i` words on `k` lines
    let mut widest = vec![vec![f64::INFINITY; n + 1]; lines + 1];
    let mut start = vec![vec![0; n + 1]; lines + 1];
    widest[0][0] = 0.;
    for k in 1..=lines {
        for i in k..=n {
            for j in (k - 1)..i {
                let candidate = widest[k - 1][j].max(line_width(j, i));
                if candidate < widest[k][i] {
                    widest[k][i] = candidate;
                    start[k][i] = j;
                }
            }
        }
    }
    let mut breaks = vec![n];
    let mut i = n;
    for k in (1..=lines).rev() {
        i = start[k][i];
        breaks.push(i);
    }
    breaks.reverse();
    (widest[lines][n], breaks)
}

/// Rounds coordinates for SVG output so files are stable and small.
fn number(value: f64) -> String {
    ((value * 100.).round() / 100.).to_string()
}

/// The `<text>` of a hint laid out by [`layout_hint`]. Lines are centered on the layout's center
/// with `dominant-baseline="central"`, and each is stretched to its measured width.
pub(crate) fn hint_element(layout: &HintLayout) -> Element {
    let metrics = &*METRICS;
    let mut text = Element::new("text");
    let (cx, cy) = layout.center;
    for (key, value) in [
        ("font-family", HINT_FONT_FAMILY.to_string()),
        ("font-size", number(layout.font_size)),
        ("text-anchor", "middle".to_string()),
        ("dominant-baseline", "central".to_string()),
        ("x", number(cx)),
        ("y", number(cy)),
    ] {
        text.attributes.insert(key.to_string(), value);
    }
    let set_length = |element: &mut Element, width: f64| {
        element
            .attributes
            .insert("textLength".to_string(), number(width));
        element
            .attributes
            .insert("lengthAdjust".to_string(), "spacingAndGlyphs".to_string());
    };
    if let [(line, width)] = layout.lines.as_slice() {
        set_length(&mut text, *width);
        text.children.push(XMLNode::Text(line.clone()));
        return text;
    }
    let line_height = metrics.line_height * layout.font_size;
    #[allow(clippy::cast_precision_loss)]
    let first_y = cy - (layout.lines.len() - 1) as f64 * line_height / 2.;
    for (i, (line, width)) in layout.lines.iter().enumerate() {
        let mut tspan = Element::new("tspan");
        tspan.attributes.insert("x".to_string(), number(cx));
        #[allow(clippy::cast_precision_loss)]
        tspan
            .attributes
            .insert("y".to_string(), number(first_y + i as f64 * line_height));
        set_length(&mut tspan, *width);
        tspan.children.push(XMLNode::Text(line.clone()));
        text.children.push(XMLNode::Element(tspan));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Element {
        Element::parse(xml.as_bytes()).unwrap()
    }

    fn limits(min: f64, max: f64) -> HintLimits {
        HintLimits {
            fill: 0.75,
            min_font_size: min,
            max_font_size: max,
            image: None,
        }
    }

    fn area(width: f64, height: f64) -> Bounds {
        Bounds {
            x: 0.,
            y: 0.,
            width,
            height,
        }
    }

    fn assert_fits(layout: &HintLayout, area: Bounds, fill: f64) {
        let widest = layout.lines.iter().map(|(_, w)| *w).fold(0., f64::max);
        #[allow(clippy::cast_precision_loss)]
        let height = layout.lines.len() as f64 * METRICS.line_height * layout.font_size;
        assert!(
            widest <= area.width * fill + 1e-6,
            "{layout:?} too wide for {area:?}"
        );
        assert!(
            height <= area.height * fill + 1e-6,
            "{layout:?} too tall for {area:?}"
        );
    }

    #[test]
    fn test_bounds_of_shapes() {
        let rect = parse(r#"<rect x="10" y="20" width="100" height="40"/>"#);
        assert_eq!(
            cloze_bounds(&rect),
            Some(Bounds {
                x: 10.,
                y: 20.,
                width: 100.,
                height: 40.
            })
        );
        // Paths had no bounds before usvg
        let path = parse(r#"<path d="M200 20 L300 40 L260 120 Z" fill="none"/>"#);
        assert_eq!(
            cloze_bounds(&path),
            Some(Bounds {
                x: 200.,
                y: 20.,
                width: 100.,
                height: 100.
            })
        );
        // Hidden and unpainted clozes are measured too
        let hidden = parse(r#"<ellipse cx="50" cy="50" rx="20" ry="10" opacity="0" fill="none"/>"#);
        assert_eq!(
            cloze_bounds(&hidden),
            Some(Bounds {
                x: 30.,
                y: 40.,
                width: 40.,
                height: 20.
            })
        );
        // A rotated square's bounds grow by √2
        let rotated =
            parse(r#"<rect x="0" y="0" width="100" height="100" transform="rotate(45 50 50)"/>"#);
        let b = cloze_bounds(&rotated).unwrap();
        assert!((b.width - 141.42).abs() < 0.1, "{b:?}");
        assert!((b.center().0 - 50.).abs() < 0.01 && (b.center().1 - 50.).abs() < 0.01);
        let group = parse(
            r#"<g id="g"><rect x="0" y="0" width="100" height="40"/><ellipse cx="200" cy="20" rx="20" ry="20"/></g>"#,
        );
        assert_eq!(
            cloze_bounds(&group),
            Some(Bounds {
                x: 0.,
                y: 0.,
                width: 220.,
                height: 40.
            })
        );
    }

    fn areas(shape: &str, image: Option<Bounds>) -> Vec<Bounds> {
        let cloze = parse(shape);
        text_areas(&cloze, cloze_bounds(&cloze).unwrap(), image)
    }

    fn largest(areas: &[Bounds]) -> Bounds {
        *areas
            .iter()
            .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)))
            .unwrap()
    }

    fn contains(outer: Bounds, inner: Bounds) -> bool {
        let e = 1e-6;
        inner.x >= outer.x - e
            && inner.y >= outer.y - e
            && inner.x + inner.width <= outer.x + outer.width + e
            && inner.y + inner.height <= outer.y + outer.height + e
    }

    #[test]
    fn test_text_area_of_rect_is_exact() {
        assert_eq!(
            areas(r#"<rect x="10" y="20" width="100" height="40"/>"#, None),
            vec![Bounds {
                x: 10.,
                y: 20.,
                width: 100.,
                height: 40.
            }]
        );
    }

    /// The largest rectangle in a circle is about a square of side r√2.
    #[test]
    fn test_text_area_of_circle_is_inscribed() {
        let a = largest(&areas(r#"<circle cx="100" cy="100" r="100"/>"#, None));
        assert!((a.width * a.height / 20_000. - 1.).abs() < 0.08, "{a:?}");
        let (cx, cy) = a.center();
        assert!((cx - 100.).abs() < 3. && (cy - 100.).abs() < 3., "{a:?}");
    }

    /// For one wide line, a flat ellipse offers a wide, low rectangle, wider than the square-ish
    /// one of largest area.
    #[test]
    fn test_wide_text_in_ellipse_uses_wide_area() {
        let all = areas(r#"<ellipse cx="200" cy="50" rx="200" ry="50"/>"#, None);
        let layout = layout_hint(
            "a long single line of text",
            &all,
            &limits(0., f64::INFINITY),
        )
        .unwrap();
        assert_eq!(layout.lines.len(), 1);
        let square_ish = largest(&all);
        assert!(
            layout.lines[0].1 > square_ish.width * 0.75,
            "{layout:?} {square_ish:?}"
        );
        let (cx, cy) = layout.center;
        assert!(
            (cx - 200.).abs() < 5. && (cy - 50.).abs() < 5.,
            "{layout:?}"
        );
    }

    /// A triangle's text goes into its fat bottom-left part, not its bounding box's center.
    #[test]
    fn test_text_area_of_triangle_is_inside() {
        let a = largest(&areas(r#"<polygon points="0,0 0,200 200,200"/>"#, None));
        // The largest rectangle under the hypotenuse is 100×100 at (0, 100)
        assert!(
            (a.width - 100.).abs() < 5. && (a.height - 100.).abs() < 5.,
            "{a:?}"
        );
        assert!(a.x < 5. && (a.y - 100.).abs() < 5., "{a:?}");
    }

    /// A rotated rect has room only in its middle, not in its bounding box's corners.
    #[test]
    fn test_text_area_of_rotated_rect_is_inside() {
        let shape =
            r#"<rect x="0" y="80" width="300" height="40" transform="rotate(30 150 100)"/>"#;
        let bounds = cloze_bounds(&parse(shape)).unwrap();
        let a = largest(&areas(shape, None));
        assert!(
            a.width * a.height < bounds.width * bounds.height / 4.,
            "{a:?} {bounds:?}"
        );
        assert!(contains(bounds, a));
    }

    #[test]
    fn test_text_area_is_kept_in_image() {
        let image = Bounds {
            x: 0.,
            y: 0.,
            width: 100.,
            height: 100.,
        };
        assert_eq!(
            areas(
                r#"<rect x="50" y="10" width="100" height="40"/>"#,
                Some(image)
            ),
            vec![Bounds {
                x: 50.,
                y: 10.,
                width: 50.,
                height: 40.
            }]
        );
        for a in areas(
            r#"<ellipse cx="100" cy="50" rx="60" ry="30"/>"#,
            Some(image),
        ) {
            assert!(contains(image, a), "{a:?}");
        }
    }

    /// Every area of a grouped cloze is inside one of its shapes, not the gap between them.
    #[test]
    fn test_text_areas_of_group_avoid_gaps() {
        let all = areas(
            r#"<g><rect x="0" y="0" width="100" height="50"/><rect x="200" y="0" width="100" height="50"/></g>"#,
            None,
        );
        let left = Bounds {
            x: 0.,
            y: 0.,
            width: 100.,
            height: 50.,
        };
        let right = Bounds {
            x: 200.,
            y: 0.,
            width: 100.,
            height: 50.,
        };
        assert!(
            all.iter()
                .all(|a| contains(left, *a) || contains(right, *a)),
            "{all:?}"
        );
    }

    #[test]
    fn test_long_hint_shrinks_to_fit() {
        let a = area(60., 20.);
        let layout = layout_hint("Supercalifragilistic", &[a], &limits(0., f64::INFINITY)).unwrap();
        assert_eq!(layout.lines.len(), 1);
        assert_fits(&layout, a, 0.75);
        // Width bound: it fills the width
        let width = layout.lines[0].1;
        assert!((width - 45.).abs() < 0.01, "{layout:?}");
    }

    #[test]
    fn test_short_hint_is_capped() {
        let a = area(2000., 1000.);
        let layout = layout_hint("A", &[a], &limits(0., 50.)).unwrap();
        assert!((layout.font_size - 50.).abs() < 1e-9);
        assert_eq!(layout.center, (1000., 500.));
    }

    #[test]
    fn test_hint_wraps_in_tall_box() {
        let a = area(80., 300.);
        let layout = layout_hint(
            "the left anterior descending artery",
            &[a],
            &limits(0., f64::INFINITY),
        )
        .unwrap();
        assert!(layout.lines.len() >= 3, "{layout:?}");
        assert_fits(&layout, a, 0.75);
        // No words lost or reordered
        let joined: Vec<_> = layout.lines.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(joined.join(" "), "the left anterior descending artery");
    }

    #[test]
    fn test_hint_stays_on_one_line_in_wide_box() {
        let layout =
            layout_hint("Hi There", &[area(146., 60.)], &limits(0., f64::INFINITY)).unwrap();
        assert_eq!(layout.lines.len(), 1);
    }

    #[test]
    fn test_minimum_size_overflows_tiny_box() {
        let layout = layout_hint("(no answer)", &[area(5., 5.)], &limits(12., 100.)).unwrap();
        assert!((layout.font_size - 12.).abs() < 1e-9);
    }

    #[test]
    fn test_blank_hint_has_no_layout() {
        assert_eq!(layout_hint("  ", &[area(10., 10.)], &limits(0., 10.)), None);
    }

    #[test]
    fn test_limits_from_config() {
        let config = ImageOcclusionConfig::default();
        let l = HintLimits::new(&config, Some((2000., 1000.)));
        assert!((l.min_font_size - 20.).abs() < 1e-9 && (l.max_font_size - 100.).abs() < 1e-9);
        let config = ImageOcclusionConfig {
            cloze_hint_font_size: Some(16),
            ..Default::default()
        };
        let l = HintLimits::new(&config, Some((2000., 1000.)));
        // The old fixed size is a ceiling, and the minimum never exceeds it
        assert!((l.max_font_size - 16.).abs() < 1e-9 && (l.min_font_size - 16.).abs() < 1e-9);
    }

    /// The hint renders inside its cloze with the bundled font: no ink outside the box.
    #[test]
    fn test_rendered_hint_ink_is_inside_cloze() {
        let a = Bounds {
            x: 20.,
            y: 30.,
            width: 120.,
            height: 50.,
        };
        let layout = layout_hint("Gyp jQ words wrap", &[a], &limits(0., f64::INFINITY)).unwrap();
        let mut buffer = Vec::new();
        hint_element(&layout)
            .write_with_config(
                &mut buffer,
                EmitterConfig::new().write_document_declaration(false),
            )
            .unwrap();
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="120">{}</svg>"#,
            String::from_utf8(buffer).unwrap()
        );
        let image = super::super::utils::render_svg_to_rgba(svg.as_bytes(), None, None).unwrap();
        let mut inked = 0;
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] > 0 {
                inked += 1;
                let (x, y) = (f64::from(x), f64::from(y));
                assert!(
                    x >= a.x && x <= a.x + a.width && y >= a.y && y <= a.y + a.height,
                    "ink at ({x}, {y}) outside {a:?}"
                );
            }
        }
        assert!(inked > 100, "hint was not drawn");
    }
}
