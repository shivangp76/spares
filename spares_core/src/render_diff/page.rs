//! The page: one section per changed note, each with three panes (the source diff, the old
//! render and the new render), as one self-contained HTML file that makes no requests, since it
//! is opened from `file://`.

use std::fmt::Write as _;

use super::pairs::ChangeKind;
use super::pairs::NoteChange;
use super::pairs::NoteLanguage;
use super::source_diff::View;
use super::source_diff::escape;
use super::source_diff::source_diff_html;
use crate::render::PreviewResult;

/// A changed note and its renders
#[derive(Clone, Debug)]
pub struct NoteDiff {
    pub change: NoteChange,
    /// The old side's render. `None` if it has no old side or wasn't rendered (Markdown).
    pub old_render: Option<PreviewResult>,
    /// The new side's render, as `old_render`
    pub new_render: Option<PreviewResult>,
}

impl NoteDiff {
    /// Whether its new side (the old side of a deleted note) fails to compile
    pub fn fails(&self) -> bool {
        let render = match self.change.kind {
            ChangeKind::Deleted => &self.old_render,
            ChangeKind::Modified | ChangeKind::Added => &self.new_render,
        };
        render.as_ref().is_some_and(|render| !render.ok)
    }

    /// Whether either side fails to compile
    fn fails_on_either_side(&self) -> bool {
        [&self.old_render, &self.new_render]
            .into_iter()
            .flatten()
            .any(|render| !render.ok)
    }
}

/// What the page's header says
#[derive(Clone, Debug, Default)]
pub struct PageInfo {
    pub title: String,
    /// A line under the title, such as the directories compared
    pub subtitle: String,
}

/// The notes in the order the page shows them: those whose new side fails to compile, then
/// modified, added and deleted notes, and last the notes whose generated links alone changed
pub fn ordered(notes: &[NoteDiff]) -> Vec<&NoteDiff> {
    let mut ordered: Vec<&NoteDiff> = notes.iter().collect();
    ordered.sort_by_key(|note| {
        let group = if note.fails() {
            0
        } else if note.change.links_only {
            4
        } else {
            match note.change.kind {
                ChangeKind::Modified => 1,
                ChangeKind::Added => 2,
                ChangeKind::Deleted => 3,
            }
        };
        (group, note.change.path.clone())
    });
    ordered
}

/// The page for `notes`
pub fn page_html(info: &PageInfo, notes: &[NoteDiff]) -> String {
    let ordered = ordered(notes);
    let count = |f: &dyn Fn(&NoteDiff) -> bool| notes.iter().filter(|note| f(note)).count();
    let counts = [
        (
            count(&|n| n.change.kind == ChangeKind::Modified && !n.change.links_only),
            "modified",
            "modified",
        ),
        (
            count(&|n| n.change.kind == ChangeKind::Added),
            "added",
            "added",
        ),
        (
            count(&|n| n.change.kind == ChangeKind::Deleted),
            "deleted",
            "deleted",
        ),
        (count(&NoteDiff::fails), "failed to compile", "error"),
        (count(&|n| n.change.links_only), "links only", "links"),
    ];
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(html, "<title>{}</title>", escape(&info.title));
    let _ = writeln!(html, "<style>{STYLE}</style>\n</head>\n<body>");
    html.push_str("<header class=\"page-head\">\n");
    let _ = writeln!(html, "<h1>{}</h1>", escape(&info.title));
    if !info.subtitle.is_empty() {
        let _ = writeln!(html, "<p class=\"subtitle\">{}</p>", escape(&info.subtitle));
    }
    html.push_str("<p class=\"counts\">");
    if notes.is_empty() {
        html.push_str("No note files changed.");
    }
    for (n, label, class) in counts {
        if n > 0 {
            let _ = write!(html, "<span class=\"badge {class}\">{n} {label}</span> ");
        }
    }
    html.push_str("</p>\n<nav class=\"jump\">");
    for (i, note) in ordered.iter().enumerate() {
        let _ = write!(
            html,
            "<a href=\"#n{i}\" class=\"{}\">{}</a> ",
            jump_class(note),
            escape(&note.change.id)
        );
    }
    html.push_str("</nav>\n");
    html.push_str(TOOLBAR);
    html.push_str(
        "<p class=\"keys\">Keys: <kbd>j</kbd>/<kbd>k</kbd> next and previous note, <kbd>e</kbd> next \
         compile error, <kbd>[</kbd>/<kbd>]</kbd> narrower and wider source, <kbd>1</kbd> \
         <kbd>2</kbd> <kbd>3</kbd> show or hide the source, old and new panes, <kbd>0</kbd> \
         reset the layout. Drag the gaps between panes to resize them (double-click to \
         reset).</p>\n</header>\n<main>\n",
    );
    let links_start = ordered
        .iter()
        .position(|note| note.change.links_only && !note.fails())
        .unwrap_or(ordered.len());
    for (i, note) in ordered.iter().enumerate() {
        if i == links_start {
            let _ = writeln!(
                html,
                "<details class=\"links-only\"><summary>Only generated links changed ({})</summary>",
                ordered.len() - links_start
            );
        }
        push_section(&mut html, i, note);
    }
    if links_start < ordered.len() {
        html.push_str("</details>\n");
    }
    let _ = write!(
        html,
        "</main>\n<script>{SCRIPT}</script>\n</body>\n</html>\n"
    );
    html
}

fn jump_class(note: &NoteDiff) -> &'static str {
    if note.fails() {
        "error"
    } else if note.change.links_only {
        "links"
    } else {
        note.change.kind.label()
    }
}

fn push_section(html: &mut String, index: usize, note: &NoteDiff) {
    let change = &note.change;
    let mut classes = String::from("note");
    if note.fails_on_either_side() {
        classes.push_str(" has-error");
    }
    let _ = write!(
        html,
        "<section class=\"{classes}\" id=\"n{index}\">\n<header><a class=\"id\" href=\"#n{index}\">{}</a>",
        escape(&change.id)
    );
    if let Some(title) = &change.title {
        let _ = write!(html, "<span class=\"title\">{}</span>", escape(title));
    }
    let _ = write!(
        html,
        "<span class=\"badge {0}\">{0}</span>",
        change.kind.label()
    );
    if change.links_only {
        html.push_str("<span class=\"badge links\">links only</span>");
    }
    for (render, side) in [(&note.old_render, "old"), (&note.new_render, "new")] {
        if render.as_ref().is_some_and(|render| !render.ok) {
            let _ = write!(
                html,
                "<span class=\"badge error\">compile error ({side})</span>"
            );
        }
    }
    let _ = write!(
        html,
        "<span class=\"path\">{}</span><span class=\"actions\">",
        escape(&change.path)
    );
    if change.kind == ChangeKind::Modified {
        html.push_str("<button type=\"button\" data-action=\"full\">Full file</button>");
    }
    if change.language == NoteLanguage::Typst {
        html.push_str("<button type=\"button\" data-action=\"wide\">Wide renders</button>");
    }
    html.push_str("</span></header>\n<div class=\"panes\">\n<div class=\"pane src\"><div class=\"pane-label\">Source</div>");
    let typst = change.language == NoteLanguage::Typst;
    let old = change.old.as_deref().unwrap_or("");
    let new = change.new.as_deref().unwrap_or("");
    let _ = write!(
        html,
        "<div class=\"view hunks\">{}</div>",
        source_diff_html(old, new, typst, View::Hunks)
    );
    if change.kind == ChangeKind::Modified {
        let _ = write!(
            html,
            "<div class=\"view full\" hidden data-htmlz=\"{}\"></div>",
            gzip_base64(&source_diff_html(old, new, typst, View::Full))
        );
    }
    html.push_str(RESIZE_HANDLE);
    html.push_str("</div>\n");
    let old_absent = (change.kind == ChangeKind::Added).then_some("new note");
    let new_absent = (change.kind == ChangeKind::Deleted).then_some("deleted");
    push_render_pane(
        html,
        "old",
        "Old",
        change,
        old_absent,
        note.old_render.as_ref(),
    );
    push_render_pane(
        html,
        "new",
        "New",
        change,
        new_absent,
        note.new_render.as_ref(),
    );
    html.push_str("</div>\n</section>\n");
}

fn push_render_pane(
    html: &mut String,
    class: &str,
    label: &str,
    change: &NoteChange,
    absent: Option<&str>,
    render: Option<&PreviewResult>,
) {
    let _ = write!(
        html,
        "<div class=\"pane render {class}\"><div class=\"pane-label\">{label}</div>"
    );
    match (absent, render) {
        (Some(absent), _) => {
            let _ = write!(html, "<p class=\"placeholder\">{absent}</p>");
        }
        (None, _) if change.language == NoteLanguage::Markdown => {
            html.push_str("<p class=\"placeholder\">Markdown notes aren't rendered</p>");
        }
        (None, None) => html.push_str("<p class=\"placeholder\">not rendered</p>"),
        (None, Some(render)) if !render.ok => {
            let _ = write!(
                html,
                "<pre class=\"error\">{}</pre>",
                escape(render.diagnostics.trim_end())
            );
        }
        (None, Some(render)) => {
            if !render.diagnostics.trim().is_empty() {
                let _ = write!(
                    html,
                    "<details class=\"warnings\"><summary>Warnings</summary><pre>{}</pre></details>",
                    escape(render.diagnostics.trim_end())
                );
            }
            html.push_str("<div class=\"pages\">");
            for (i, page) in render.pages.iter().enumerate() {
                let (width, height) = svg_size(page);
                let _ = write!(
                    html,
                    "<img alt=\"{label} page {}\" width=\"{width}\" height=\"{height}\" \
                     data-svgz=\"{}\">",
                    i + 1,
                    gzip_base64(page)
                );
            }
            html.push_str("</div>");
        }
    }
    if class == "old" {
        html.push_str(RESIZE_HANDLE);
    }
    html.push_str("</div>\n");
}

/// The handle at a pane's right edge that resizes it and the next pane shown
const RESIZE_HANDLE: &str =
    "<div class=\"resize\" title=\"Drag to resize, double-click to reset\"></div>";

/// The layout controls, which apply to every note and are remembered by the browser
const TOOLBAR: &str = "<div class=\"toolbar\"><span class=\"group\">Panes: \
<label><input type=\"checkbox\" data-pane=\"src\" checked> Source</label> \
<label><input type=\"checkbox\" data-pane=\"old\" checked> Old</label> \
<label><input type=\"checkbox\" data-pane=\"new\" checked> New</label></span>\
<label><input type=\"checkbox\" data-option=\"stacked\"> Source above renders</label>\
<label><input type=\"checkbox\" data-option=\"full\"> Full files</label>\
<span class=\"group\">Source width: <input type=\"range\" data-option=\"src-width\" min=\"10\" max=\"70\" \
step=\"1\" aria-label=\"Source width\"> <output></output></span>\
<button type=\"button\" data-option=\"reset\">Reset layout</button></div>\n";

/// `text` gzipped, in base64, for `SCRIPT` to inflate when it is needed: a rendered page once it
/// is near the viewport, shown as an image, and a full-file source view once it is asked for.
/// Typst's SVGs repeat a `<use>` element per glyph, so a note's page is about 200 KB, and a few
/// hundred notes would make a page of hundreds of MB; gzip makes them about a seventh. Each page
/// is an image of its own, so the ids of different pages' glyphs can't clash, and its links and
/// scripts don't run.
fn gzip_base64(text: &str) -> String {
    use std::io::Write as _;

    use base64::Engine as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a `Vec` doesn't fail
    let _ = encoder.write_all(text.as_bytes());
    let gzipped = encoder.finish().unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(gzipped)
}

/// The size of an SVG page, from its `viewBox`, rounded, so its image takes its place before it
/// is shown. A4 if it has none.
fn svg_size(svg: &str) -> (u32, u32) {
    let size = || {
        let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
        let end = start + svg[start..].find('"')?;
        let numbers: Vec<f64> = svg[start..end]
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        match numbers[..] {
            [_, _, width, height] if width > 0.0 && height > 0.0 => {
                // Pages are well within u32
                #[allow(clippy::cast_sign_loss)]
                Some((width.round() as u32, height.round() as u32))
            }
            _ => None,
        }
    };
    size().unwrap_or((595, 842))
}

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #ffffff; --fg: #1f2328; --muted: #656d76; --border: #d0d7de; --pane: #f6f8fa;
  --del: #ffebe9; --del-em: #ffb3ad; --add: #dafbe1; --add-em: #8be29b;
  --hunk: #ddf4ff; --hunk-fg: #0550ae; --error: #fff1f0; --error-fg: #a40e26;
  --b-modified: #0969da; --b-added: #1a7f37; --b-deleted: #cf222e; --b-error: #a40e26; --b-links: #6e7781;
  --t-comment: #6e7781; --t-func: #0550ae; --t-key: #8250df; --t-str: #0a3069; --t-num: #0550ae;
  --t-math: #953800; --t-op: #cf222e; --t-escape: #116329; --t-label: #116329; --t-punct: #57606a;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117; --fg: #e6edf3; --muted: #8d96a0; --border: #30363d; --pane: #161b22;
    --del: rgba(248, 81, 73, 0.16); --del-em: rgba(248, 81, 73, 0.45);
    --add: rgba(46, 160, 67, 0.16); --add-em: rgba(46, 160, 67, 0.45);
    --hunk: rgba(56, 139, 253, 0.15); --hunk-fg: #79c0ff; --error: rgba(248, 81, 73, 0.12); --error-fg: #ff7b72;
    --b-modified: #4493f8; --b-added: #3fb950; --b-deleted: #f85149; --b-error: #ff7b72; --b-links: #8d96a0;
    --t-comment: #8d96a0; --t-func: #79c0ff; --t-key: #d2a8ff; --t-str: #a5d6ff; --t-num: #79c0ff;
    --t-math: #ffa657; --t-op: #ff7b72; --t-escape: #7ee787; --t-label: #7ee787; --t-punct: #8d96a0;
  }
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; }
.page-head { padding: 16px 16px 8px; border-bottom: 1px solid var(--border); }
h1 { font-size: 20px; margin: 0 0 4px; }
.subtitle, .keys { color: var(--muted); margin: 4px 0; overflow-wrap: anywhere; }
.counts { margin: 8px 0; }
.jump { display: flex; flex-wrap: wrap; gap: 4px 10px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; }
.jump a { color: var(--fg); text-decoration: none; border-bottom: 2px solid var(--b-modified); }
.jump a.added { border-color: var(--b-added); } .jump a.deleted { border-color: var(--b-deleted); }
.jump a.error { border-color: var(--b-error); color: var(--b-error); } .jump a.links { border-color: var(--b-links); }
kbd { font: 11px ui-monospace, Menlo, monospace; border: 1px solid var(--border); border-radius: 4px; padding: 0 4px; }
main { padding: 0 16px 48px; }
section.note { margin: 16px 0 28px; }
section.note > header { position: sticky; top: 0; z-index: 2; background: var(--bg); display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; padding: 8px 0; border-bottom: 1px solid var(--border); }
.id { font: 600 14px ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--fg); text-decoration: none; }
.title { font-weight: 600; }
.path { color: var(--muted); font: 12px ui-monospace, Menlo, monospace; }
.actions { margin-left: auto; display: flex; gap: 6px; }
button { font: inherit; font-size: 12px; color: var(--fg); background: var(--pane); border: 1px solid var(--border); border-radius: 6px; padding: 2px 8px; cursor: pointer; }
button[aria-pressed="true"] { background: var(--hunk); }
.badge { display: inline-block; font-size: 12px; line-height: 18px; padding: 0 7px; border-radius: 9px; border: 1px solid currentColor; color: var(--b-modified); }
.badge.added { color: var(--b-added); } .badge.deleted { color: var(--b-deleted); }
.badge.error { color: var(--b-error); } .badge.links { color: var(--b-links); }
.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 16px; margin: 10px 0 4px; font-size: 13px; }
.toolbar .group { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; }
.toolbar label { display: inline-flex; align-items: center; gap: 4px; cursor: pointer; white-space: nowrap; }
.toolbar input[type="range"] { width: 140px; }
.toolbar output { font-variant-numeric: tabular-nums; min-width: 3.5em; color: var(--muted); }
/* `--cols` is set by SCRIPT from the panes shown and their widths */
.panes { display: grid; grid-template-columns: var(--cols, minmax(0, 1.2fr) minmax(0, 1fr) minmax(0, 1fr)); gap: 12px; margin-top: 10px; }
.pane { position: relative; min-width: 0; }
.resize { position: absolute; top: 0; bottom: 0; right: -10px; width: 8px; cursor: col-resize; z-index: 1; border-radius: 4px; touch-action: none; }
.resize:hover, .resizing .resize.active { background: var(--hunk); box-shadow: inset 0 0 0 1px var(--hunk-fg); }
.resizing { cursor: col-resize; user-select: none; -webkit-user-select: none; }
.hide-src .pane.src, .hide-old .pane.old, .hide-new .pane.new { display: none; }
.pane.new .resize, .hide-new .pane.old .resize, .hide-old.hide-new .pane.src .resize { display: none; }
.stacked .pane.src { grid-column: 1 / -1; } .stacked .pane.src .resize { display: none; }
@media (max-width: 1100px) { .panes { grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); } .pane.src { grid-column: 1 / -1; } .resize { display: none; } }
@media (max-width: 640px) { .panes { grid-template-columns: minmax(0, 1fr); } }
section.note.wide .panes { grid-template-columns: minmax(0, 1fr); }
section.note.wide .pane.src { grid-column: auto; } section.note.wide .resize { display: none; }
.pane-label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--muted); margin-bottom: 4px; }
.diff { font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; border: 1px solid var(--border); border-radius: 6px; overflow: hidden; }
.hunk + .hunk { border-top: 1px solid var(--border); }
.hunk-head { background: var(--hunk); color: var(--hunk-fg); padding: 2px 8px; }
.l { display: grid; grid-template-columns: calc(var(--digits) + 10px) calc(var(--digits) + 10px) minmax(0, 1fr); }
.l.del { background: var(--del); } .l.add { background: var(--add); }
.n { color: var(--muted); text-align: right; padding-right: 6px; user-select: none; -webkit-user-select: none; }
.c { white-space: pre-wrap; overflow-wrap: anywhere; padding: 0 8px; min-height: 1.5em; }
.del .em { background: var(--del-em); border-radius: 2px; } .add .em { background: var(--add-em); border-radius: 2px; }
.pages img { display: block; width: 100%; height: auto; background: #fff; border: 1px solid var(--border); box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12); margin-bottom: 10px; }
.placeholder { color: var(--muted); font-style: italic; border: 1px dashed var(--border); border-radius: 6px; padding: 24px; text-align: center; margin: 0; }
pre.error, .warnings pre { white-space: pre-wrap; overflow-wrap: anywhere; font: 12px/1.45 ui-monospace, Menlo, monospace; margin: 0; padding: 10px; border-radius: 6px; }
pre.error { background: var(--error); color: var(--error-fg); border: 1px solid var(--error-fg); }
.warnings { margin-bottom: 8px; font-size: 12px; color: var(--muted); } .warnings pre { background: var(--pane); color: var(--fg); }
details.links-only > summary { cursor: pointer; font-weight: 600; margin: 20px 0 8px; }
.typ-comment { color: var(--t-comment); font-style: italic; }
.typ-func, .typ-heading, .typ-num { color: var(--t-func); }
.typ-key, .typ-pol { color: var(--t-key); }
.typ-str, .typ-raw { color: var(--t-str); }
.typ-math-delim, .typ-marker { color: var(--t-math); }
.typ-math-op, .typ-op { color: var(--t-op); }
.typ-escape, .typ-label, .typ-ref { color: var(--t-escape); }
.typ-punct { color: var(--t-punct); }
.typ-strong, .typ-heading, .typ-term { font-weight: 600; }
.typ-emph { font-style: italic; }
.typ-link { text-decoration: underline; }
"#;

const SCRIPT: &str = r"
(function () {
  // Pages are gzipped SVGs, shown as images when near the viewport
  const gunzip = (base64) => {
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    return new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip')));
  };
  const inflate = async (img) => {
    const svg = await gunzip(img.dataset.svgz).arrayBuffer();
    delete img.dataset.svgz;
    img.src = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' }));
  };
  const near = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting) {
        near.unobserve(entry.target);
        inflate(entry.target);
      }
    }
  }, { rootMargin: '2000px 0px' });
  document.querySelectorAll('img[data-svgz]').forEach((img) => near.observe(img));
  const visible = () => Array.from(document.querySelectorAll('section.note')).filter((n) => n.offsetParent !== null);
  const top = (n) => n.getBoundingClientRect().top + window.scrollY;
  const current = (notes) => {
    const y = window.scrollY + 2;
    let i = -1;
    notes.forEach((n, k) => { if (top(n) <= y) i = k; });
    return i;
  };
  const go = (n) => {
    const details = n && n.closest('details');
    if (details) details.open = true;
    if (n) n.scrollIntoView({ block: 'start' });
  };
  // The layout, for every note: the panes shown and their widths, which the browser remembers
  const root = document.documentElement;
  const PANES = ['src', 'old', 'new'];
  const DEFAULT_WIDTHS = { src: 1.2, old: 1, new: 1 };
  const STORAGE_KEY = 'spares-render-diff-layout';
  const layout = { widths: { ...DEFAULT_WIDTHS }, hidden: [], stacked: false, full: false };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || '{}');
    for (const pane of PANES) {
      const width = saved.widths && saved.widths[pane];
      if (typeof width === 'number' && width > 0) layout.widths[pane] = width;
    }
    if (Array.isArray(saved.hidden)) layout.hidden = saved.hidden.filter((pane) => PANES.includes(pane)).slice(0, 2);
    layout.stacked = saved.stacked === true;
    layout.full = saved.full === true;
  } catch (_) {}
  const save = () => {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(layout)); } catch (_) {}
  };
  const shown = () => PANES.filter((pane) => !layout.hidden.includes(pane));
  // The panes in the grid's columns: a stacked source pane spans them all
  const columns = () => shown().filter((pane) => !(layout.stacked && pane === 'src'));
  const toolbar = document.querySelector('.toolbar');
  const slider = toolbar.querySelector('[data-option=src-width]');
  const sourceShare = () => {
    const panes = columns();
    const total = panes.reduce((sum, pane) => sum + layout.widths[pane], 0);
    return panes.includes('src') && panes.length > 1 ? layout.widths.src / total : null;
  };
  const applyLayout = () => {
    const panes = columns();
    root.style.setProperty('--cols', panes.length
      ? panes.map((pane) => `minmax(0, ${layout.widths[pane].toFixed(4)}fr)`).join(' ')
      : 'minmax(0, 1fr)');
    for (const pane of PANES) root.classList.toggle('hide-' + pane, layout.hidden.includes(pane));
    root.classList.toggle('stacked', layout.stacked);
    toolbar.querySelectorAll('[data-pane]').forEach((box) => { box.checked = !layout.hidden.includes(box.dataset.pane); });
    toolbar.querySelector('[data-option=stacked]').checked = layout.stacked;
    toolbar.querySelector('[data-option=full]').checked = layout.full;
    const share = sourceShare();
    slider.disabled = share === null;
    if (share !== null) slider.value = String(Math.round(share * 100));
    slider.nextElementSibling.textContent = share === null ? '' : Math.round(share * 100) + '%';
  };
  // Changes the layout without moving the note at the top of the window, which changing the
  // widths of its renders would otherwise do
  let pending = null;
  const relayout = (anchor, then) => {
    anchor = anchor || visible()[Math.max(current(visible()), 0)];
    const before = anchor ? anchor.getBoundingClientRect().top : 0;
    applyLayout();
    if (then) then();
    if (anchor) window.scrollBy(0, anchor.getBoundingClientRect().top - before);
  };
  const relayoutSoon = (anchor) => {
    if (pending) return;
    pending = requestAnimationFrame(() => { pending = null; relayout(anchor); });
  };
  const setSourceShare = (percent) => {
    const others = columns().filter((pane) => pane !== 'src');
    if (!others.length || !columns().includes('src')) return;
    const share = Math.min(Math.max(percent, 10), 70) / 100;
    const rest = others.reduce((sum, pane) => sum + layout.widths[pane], 0);
    layout.widths.src = share / (1 - share) * rest;
  };
  const togglePane = (pane, on) => {
    const hidden = layout.hidden.filter((p) => p !== pane);
    if (!on) hidden.push(pane);
    // At least one pane stays
    if (hidden.length < PANES.length) layout.hidden = hidden;
  };
  const showFull = async (note, on) => {
    const button = note.querySelector('button[data-action=full]');
    if (!button) return;
    button.setAttribute('aria-pressed', String(on));
    const full = note.querySelector('.view.full');
    if (on && full.dataset.htmlz) {
      // The source pane's own markup, gzipped
      full.innerHTML = await gunzip(full.dataset.htmlz).text();
      delete full.dataset.htmlz;
    }
    note.querySelector('.view.hunks').hidden = on;
    full.hidden = !on;
  };
  const showAllFull = (on) => {
    const anchor = visible()[Math.max(current(visible()), 0)];
    const before = anchor ? anchor.getBoundingClientRect().top : 0;
    Promise.all(Array.from(document.querySelectorAll('section.note')).map((note) => showFull(note, on))).then(() => {
      if (anchor) window.scrollBy(0, anchor.getBoundingClientRect().top - before);
    });
  };
  applyLayout();
  if (layout.full) showAllFull(true);
  toolbar.addEventListener('input', (event) => {
    const input = event.target;
    if (input.dataset.pane) {
      togglePane(input.dataset.pane, input.checked);
    } else if (input.dataset.option === 'stacked') {
      layout.stacked = input.checked;
    } else if (input.dataset.option === 'full') {
      layout.full = input.checked;
      showAllFull(input.checked);
    } else if (input.dataset.option === 'src-width') {
      setSourceShare(Number(input.value));
      relayoutSoon();
      return;
    }
    relayout();
    save();
  });
  slider.addEventListener('change', save);
  toolbar.querySelector('[data-option=reset]').addEventListener('click', () => {
    Object.assign(layout, { widths: { ...DEFAULT_WIDTHS }, hidden: [], stacked: false });
    if (layout.full) { layout.full = false; showAllFull(false); }
    relayout();
    save();
  });
  // Dragging a pane's right edge moves width between it and the next pane shown
  document.addEventListener('pointerdown', (event) => {
    const handle = event.target.closest('.resize');
    if (!handle || event.button !== 0) return;
    const pane = handle.closest('.pane');
    const name = PANES.find((p) => pane.classList.contains(p));
    const panes = columns();
    const next = panes[panes.indexOf(name) + 1];
    if (!next) return;
    const nextPane = pane.parentElement.querySelector('.pane.' + next);
    const a = pane.getBoundingClientRect().width;
    const b = nextPane.getBoundingClientRect().width;
    const fr = layout.widths[name] + layout.widths[next];
    const min = Math.min(120, (a + b) / 2);
    const startX = event.clientX;
    const section = handle.closest('section.note');
    event.preventDefault();
    handle.setPointerCapture(event.pointerId);
    handle.classList.add('active');
    root.classList.add('resizing');
    const move = (e) => {
      const width = Math.min(Math.max(a + e.clientX - startX, min), a + b - min);
      layout.widths[name] = fr * width / (a + b);
      layout.widths[next] = fr - layout.widths[name];
      relayoutSoon(section);
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
      handle.classList.remove('active');
      root.classList.remove('resizing');
      save();
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  });
  document.addEventListener('dblclick', (event) => {
    const handle = event.target.closest('.resize');
    if (!handle) return;
    layout.widths = { ...DEFAULT_WIDTHS };
    relayout(handle.closest('section.note'));
    save();
  });
  document.addEventListener('keydown', (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (event.target.closest && event.target.closest('input, textarea, select')) return;
    const notes = visible();
    const i = current(notes);
    if (event.key === '[' || event.key === ']') {
      const share = sourceShare();
      if (share === null) return;
      setSourceShare(Math.round(share * 100) + (event.key === '[' ? -5 : 5));
      relayout();
      save();
    } else if (event.key === '1' || event.key === '2' || event.key === '3') {
      const pane = PANES[Number(event.key) - 1];
      togglePane(pane, layout.hidden.includes(pane));
      relayout();
      save();
    } else if (event.key === '0') {
      layout.widths = { ...DEFAULT_WIDTHS };
      layout.hidden = [];
      layout.stacked = false;
      relayout();
      save();
    } else if (event.key === 'j') {
      go(notes[Math.min(i + 1, notes.length - 1)]);
    } else if (event.key === 'k') {
      const back = i >= 0 && window.scrollY > top(notes[i]) + 4 ? i : i - 1;
      go(notes[Math.max(back, 0)]);
    } else if (event.key === 'e') {
      const all = Array.from(document.querySelectorAll('section.note'));
      const from = i >= 0 ? all.indexOf(notes[i]) : -1;
      go(all.slice(from + 1).find((n) => n.classList.contains('has-error')) || all.find((n) => n.classList.contains('has-error')));
    } else {
      return;
    }
    event.preventDefault();
  });
  document.addEventListener('click', (event) => {
    const button = event.target.closest('button[data-action]');
    if (!button) return;
    const note = button.closest('section.note');
    const on = button.getAttribute('aria-pressed') !== 'true';
    if (button.dataset.action === 'full') {
      showFull(note, on);
    } else if (button.dataset.action === 'wide') {
      button.setAttribute('aria-pressed', String(on));
      note.classList.toggle('wide', on);
    }
  });
})();
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_diff::pairs::note_change;

    fn note(path: &str, old: Option<&str>, new: Option<&str>) -> NoteChange {
        note_change(
            path.to_string(),
            NoteLanguage::of_path(path).unwrap(),
            old.map(String::from),
            new.map(String::from),
        )
        .unwrap()
    }

    fn rendered(ok: bool) -> PreviewResult {
        PreviewResult {
            name: String::new(),
            ok,
            diagnostics: if ok {
                String::new()
            } else {
                "error: unknown variable: <x>".to_string()
            },
            pages: if ok {
                vec![
                    "<svg viewBox=\"0 0 595.5 842\" xmlns=\"http://www.w3.org/2000/svg\"><path \
                      fill=\"#000\"/></svg>"
                        .to_string(),
                ]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn sections_are_ordered_and_labelled() {
        let links = "a\n// spares: note end\n\nLinked Notes:\n+ x\n\n// spares: end\n";
        let notes = vec![
            NoteDiff {
                change: note("typst/del.typ", Some("gone"), None),
                old_render: Some(rendered(true)),
                new_render: None,
            },
            NoteDiff {
                change: note(
                    "typst/mod.typ",
                    Some("#key[Mod]\nold"),
                    Some("#key[Mod]\nnew"),
                ),
                old_render: Some(rendered(true)),
                new_render: Some(rendered(true)),
            },
            NoteDiff {
                change: note(
                    "typst/lnk.typ",
                    Some(links),
                    Some(&links.replace("+ x", "+ y")),
                ),
                old_render: Some(rendered(true)),
                new_render: Some(rendered(true)),
            },
            NoteDiff {
                change: note("typst/add.typ", None, Some("new")),
                old_render: None,
                new_render: Some(rendered(true)),
            },
            NoteDiff {
                change: note("typst/bad.typ", Some("fine"), Some("#x")),
                old_render: Some(rendered(true)),
                new_render: Some(rendered(false)),
            },
            NoteDiff {
                change: note("markdown/md.md", Some("a"), Some("b")),
                old_render: None,
                new_render: None,
            },
        ];
        let order: Vec<&str> = ordered(&notes)
            .iter()
            .map(|n| n.change.id.as_str())
            .collect();
        assert_eq!(order, ["bad", "md", "mod", "add", "del", "lnk"]);

        let html = page_html(
            &PageInfo {
                title: "Render diff".to_string(),
                subtitle: "left → right".to_string(),
            },
            &notes,
        );
        let position = |needle: &str| html.find(needle).unwrap_or_else(|| panic!("{needle}"));
        assert!(position("id=\"n0\">") < position("id=\"n5\">"));
        assert!(html.contains("<span class=\"badge modified\">3 modified</span>"));
        assert!(html.contains("<span class=\"badge error\">1 failed to compile</span>"));
        assert!(html.contains("<span class=\"badge links\">1 links only</span>"));
        assert!(html.contains("<section class=\"note has-error\" id=\"n0\">"));
        assert!(html.contains("compile error (new)"));
        assert!(html.contains("<pre class=\"error\">error: unknown variable: &lt;x&gt;</pre>"));
        assert!(html.contains("<p class=\"placeholder\">new note</p>"));
        assert!(html.contains("<p class=\"placeholder\">deleted</p>"));
        assert!(html.contains("Markdown notes aren't rendered"));
        assert!(html.contains("<span class=\"title\">Mod</span>"));
        assert!(position("<details class=\"links-only\">") < position("id=\"n5\">"));
        assert!(position("id=\"n4\">") < position("<details class=\"links-only\">"));
        // Pages are gzipped SVGs, sized by their viewBox
        let start = position("data-svgz=\"") + "data-svgz=\"".len();
        let end = start + html[start..].find('"').unwrap();
        let gzipped = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &html[start..end],
        )
        .unwrap();
        let mut svg = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&gzipped[..]), &mut svg)
            .unwrap();
        assert_eq!(svg, rendered(true).pages[0]);
        assert!(
            html.contains("width=\"596\" height=\"842\" data-svgz="),
            "{html}"
        );
        // No requests from file://
        assert!(!html.contains("https://"));
        // The layout controls, and handles to resize the source and old panes of every note
        assert!(html.contains("<input type=\"checkbox\" data-pane=\"src\" checked>"));
        assert!(html.contains("data-option=\"src-width\""));
        assert_eq!(
            html.matches("<div class=\"resize\"").count(),
            2 * notes.len()
        );
    }
}
