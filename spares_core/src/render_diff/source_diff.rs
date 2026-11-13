//! The source pane: a unified diff of a note's text as HTML, in the style of delta. Removed and
//! added lines have red and green backgrounds, the words that changed within a line a stronger
//! one, and Typst is syntax highlighted.

use std::fmt::Write as _;
use std::ops::Range;

use typst_syntax::LinkedNode;
use typst_syntax::Tag;

/// Lines of context around each change
const CONTEXT_LINES: usize = 3;

/// A removed and an added line are shown as one line edited (their changed words emphasised) when
/// their distance, from 0 (the same) to 1 (nothing in common), is at most this. delta's default
/// `max-line-distance`.
const MAX_LINE_DISTANCE: f64 = 0.6;

/// Lines longer than this many tokens, multiplied together, aren't compared word by word
const MAX_WORD_DIFF_CELLS: usize = 4_000_000;

/// How the source pane shows a file
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    /// Changed lines with [`CONTEXT_LINES`] around them, in hunks
    Hunks,
    /// The whole file, with the changes in place
    Full,
}

/// The diff from `old` to `new` as HTML: hunks of lines, each with its old and new line numbers.
/// `typst` turns on syntax highlighting. An added or deleted file is diffed against nothing.
pub fn source_diff_html(old: &str, new: &str, typst: bool, view: View) -> String {
    let old_side = Side::new(old, typst);
    let new_side = Side::new(new, typst);
    let context = match view {
        View::Hunks => CONTEXT_LINES,
        View::Full => old_side.lines.len().max(new_side.lines.len()) + 1,
    };
    let patch = diffy::DiffOptions::new()
        .set_context_len(context)
        .create_patch(old, new);
    let digits = old_side
        .lines
        .len()
        .max(new_side.lines.len())
        .max(1)
        .to_string()
        .len();
    let mut html = format!("<div class=\"diff\" style=\"--digits:{digits}ch\">\n");
    for hunk in patch.hunks() {
        let (old_range, new_range) = (hunk.old_range(), hunk.new_range());
        let _ = write!(
            html,
            "<div class=\"hunk\">\n<div class=\"hunk-head\">@@ -{},{} +{},{} @@</div>\n",
            old_range.start(),
            old_range.len(),
            new_range.start(),
            new_range.len()
        );
        // 0-based line indices
        let first = |range: diffy::HunkRange| range.start().saturating_sub(1);
        let mut old_line = first(old_range);
        let mut new_line = first(new_range);
        let lines = hunk.lines();
        let mut i = 0;
        while i < lines.len() {
            match lines[i] {
                diffy::Line::Context(_) => {
                    push_line(
                        &mut html,
                        LineKind::Context,
                        Some(old_line),
                        Some(new_line),
                        &old_side,
                        &[],
                    );
                    old_line += 1;
                    new_line += 1;
                    i += 1;
                }
                diffy::Line::Delete(_) | diffy::Line::Insert(_) => {
                    // A block of changes: its removed lines, then its added lines
                    let removed =
                        count_while(&lines[i..], |line| matches!(line, diffy::Line::Delete(_)));
                    let added = count_while(&lines[i + removed..], |line| {
                        matches!(line, diffy::Line::Insert(_))
                    });
                    let removed_lines: Vec<usize> = (old_line..old_line + removed).collect();
                    let added_lines: Vec<usize> = (new_line..new_line + added).collect();
                    let emphasis =
                        emphasise_changes(&old_side, &removed_lines, &new_side, &added_lines);
                    for (k, &line) in removed_lines.iter().enumerate() {
                        push_line(
                            &mut html,
                            LineKind::Removed,
                            Some(line),
                            None,
                            &old_side,
                            &emphasis.removed[k],
                        );
                    }
                    for (k, &line) in added_lines.iter().enumerate() {
                        push_line(
                            &mut html,
                            LineKind::Added,
                            None,
                            Some(line),
                            &new_side,
                            &emphasis.added[k],
                        );
                    }
                    old_line += removed;
                    new_line += added;
                    i += removed + added;
                }
            }
        }
        html.push_str("</div>\n");
    }
    html.push_str("</div>");
    html
}

fn count_while<T>(items: &[T], f: impl Fn(&T) -> bool) -> usize {
    items.iter().take_while(|item| f(item)).count()
}

/// One side of the diff: its text, where its lines are, and its syntax highlighting
struct Side<'a> {
    text: &'a str,
    /// Each line's byte range, without its line break
    lines: Vec<Range<usize>>,
    /// Highlighted byte ranges and their classes, in order and not overlapping
    highlights: Vec<(Range<usize>, String)>,
}

impl<'a> Side<'a> {
    fn new(text: &'a str, typst: bool) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let content = content.strip_suffix('\r').unwrap_or(content);
            lines.push(start..start + content.len());
            start += line.len();
        }
        let highlights = if typst { highlights(text) } else { Vec::new() };
        Self {
            text,
            lines,
            highlights,
        }
    }

    fn line(&self, index: usize) -> &'a str {
        self.lines
            .get(index)
            .map_or("", |range| &self.text[range.clone()])
    }
}

/// The CSS classes of each leaf of the Typst syntax tree of `text` that has any: the tags of the
/// leaf and of the nodes around it (`typ-strong typ-func` for a function call in bold), so the
/// spans don't nest
fn highlights(text: &str) -> Vec<(Range<usize>, String)> {
    fn walk(
        node: &LinkedNode,
        classes: &mut Vec<&'static str>,
        out: &mut Vec<(Range<usize>, String)>,
    ) {
        let tag = typst_syntax::highlight(node).filter(|tag| *tag != Tag::Error);
        if let Some(tag) = tag {
            classes.push(tag.css_class());
        }
        if node.children().len() == 0 {
            if !classes.is_empty() && !node.leaf_text().is_empty() {
                out.push((node.range(), classes.join(" ")));
            }
        } else {
            for child in node.children() {
                walk(&child, classes, out);
            }
        }
        if tag.is_some() {
            classes.pop();
        }
    }
    let root = typst_syntax::parse(text);
    let mut out = Vec::new();
    walk(&LinkedNode::new(&root), &mut Vec::new(), &mut out);
    out
}

#[derive(Clone, Copy)]
enum LineKind {
    Context,
    Removed,
    Added,
}

/// Appends a line: its numbers, then its text, split wherever its highlighting or emphasis
/// changes. `emphasis` holds byte ranges within the line.
fn push_line(
    html: &mut String,
    kind: LineKind,
    old_number: Option<usize>,
    new_number: Option<usize>,
    side: &Side,
    emphasis: &[Range<usize>],
) {
    // Context lines are read from the old side
    let index = match kind {
        LineKind::Context | LineKind::Removed => old_number,
        LineKind::Added => new_number,
    }
    .unwrap_or(0);
    let class = match kind {
        LineKind::Context => "l",
        LineKind::Removed => "l del",
        LineKind::Added => "l add",
    };
    let number = |n: Option<usize>| n.map(|n| (n + 1).to_string()).unwrap_or_default();
    let _ = write!(
        html,
        "<div class=\"{class}\"><span class=\"n\">{}</span><span class=\"n\">{}</span><span class=\"c\">",
        number(old_number),
        number(new_number),
    );
    let range = side.lines.get(index).cloned().unwrap_or(0..0);
    push_spans(html, side, range, emphasis);
    html.push_str("</span></div>\n");
}

/// The text of `range` in `side`, as spans of its highlight classes, with the ranges of
/// `emphasis` (relative to `range`) in `em` spans
fn push_spans(html: &mut String, side: &Side, range: Range<usize>, emphasis: &[Range<usize>]) {
    let start = range.start;
    let mut cuts: Vec<usize> = vec![range.start, range.end];
    let first = side
        .highlights
        .partition_point(|(r, _)| r.end <= range.start);
    let highlights: Vec<&(Range<usize>, String)> = side.highlights[first..]
        .iter()
        .take_while(|(r, _)| r.start < range.end)
        .collect();
    for (r, _) in &highlights {
        cuts.push(r.start.clamp(range.start, range.end));
        cuts.push(r.end.clamp(range.start, range.end));
    }
    for r in emphasis {
        cuts.push((start + r.start).min(range.end));
        cuts.push((start + r.end).min(range.end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    // Runs of text with the same classes, merged
    let mut segments: Vec<(String, Range<usize>)> = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if side.text.get(a..b).is_none() {
            continue;
        }
        let highlight = highlights
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, classes)| classes.as_str());
        let emphasised = emphasis
            .iter()
            .any(|r| start + r.start <= a && b <= start + r.end);
        let classes = match (emphasised, highlight) {
            (true, Some(highlight)) => format!("em {highlight}"),
            (true, None) => "em".to_string(),
            (false, Some(highlight)) => highlight.to_string(),
            (false, None) => String::new(),
        };
        match segments.last_mut() {
            Some((last, range)) if *last == classes && range.end == a => range.end = b,
            _ => segments.push((classes, a..b)),
        }
    }
    for (classes, range) in segments {
        let text = &side.text[range];
        if classes.is_empty() {
            push_escaped(html, text);
        } else {
            let _ = write!(html, "<span class=\"{classes}\">");
            push_escaped(html, text);
            html.push_str("</span>");
        }
    }
}

/// Appends `text`, escaped for HTML
pub(super) fn push_escaped(html: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '&' => html.push_str("&amp;"),
            '"' => html.push_str("&quot;"),
            '\'' => html.push_str("&#39;"),
            c => html.push(c),
        }
    }
}

/// `text`, escaped for HTML
pub(super) fn escape(text: &str) -> String {
    let mut html = String::with_capacity(text.len());
    push_escaped(&mut html, text);
    html
}

/// The changed words of each line of a block of changes, as byte ranges within the line. Empty for
/// a line that wasn't paired with one on the other side.
#[derive(Debug, Default, PartialEq)]
struct Emphasis {
    removed: Vec<Vec<Range<usize>>>,
    added: Vec<Vec<Range<usize>>>,
}

fn emphasise_changes(old: &Side, removed: &[usize], new: &Side, added: &[usize]) -> Emphasis {
    let removed_text: Vec<&str> = removed.iter().map(|&i| old.line(i)).collect();
    let added_text: Vec<&str> = added.iter().map(|&i| new.line(i)).collect();
    let mut emphasis = Emphasis {
        removed: vec![Vec::new(); removed.len()],
        added: vec![Vec::new(); added.len()],
    };
    for (r, a, diff) in pair_lines(&removed_text, &added_text) {
        emphasis.removed[r] = diff.old;
        emphasis.added[a] = diff.new;
    }
    emphasis
}

/// The changed tokens of two lines
#[derive(Debug, Default, PartialEq)]
struct WordDiff {
    old: Vec<Range<usize>>,
    new: Vec<Range<usize>>,
    /// From 0 (the same) to 1 (nothing in common)
    distance: f64,
}

/// Pairs removed lines with added lines as delta does: in order, each removed line with the first
/// added line after the last one paired whose distance is at most [`MAX_LINE_DISTANCE`]. Returns
/// the indices of each pair and their word diff.
fn pair_lines(removed: &[&str], added: &[&str]) -> Vec<(usize, usize, WordDiff)> {
    let mut pairs = Vec::new();
    let mut next_added = 0;
    for (r, old) in removed.iter().enumerate() {
        for (a, new) in added.iter().enumerate().skip(next_added) {
            if let Some(diff) = word_diff(old, new)
                && diff.distance <= MAX_LINE_DISTANCE
            {
                pairs.push((r, a, diff));
                next_added = a + 1;
                break;
            }
        }
    }
    pairs
}

/// Splits a line into tokens, as byte ranges: runs of letters and digits, runs of whitespace, and
/// single other characters, so that `$f_(x y)$` is `$`, `f`, `_`, `(`, `x`, ` `, `y`, `)`, `$`
fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let class = |c: char| {
        if c.is_alphanumeric() {
            1
        } else if c.is_whitespace() {
            2
        } else {
            0
        }
    };
    let mut previous = None;
    for (i, c) in line.char_indices() {
        let current = class(c);
        match tokens.last_mut() {
            Some(last) if current != 0 && previous == Some(current) => last.end = i + c.len_utf8(),
            _ => tokens.push(i..i + c.len_utf8()),
        }
        previous = Some(current);
    }
    tokens
}

/// The tokens of `old` and `new` that aren't in their longest common subsequence, merged into
/// ranges. `None` if the lines are too long to compare.
fn word_diff(old: &str, new: &str) -> Option<WordDiff> {
    let (a, b) = (tokens(old), tokens(new));
    if a.len().saturating_mul(b.len()) > MAX_WORD_DIFF_CELLS {
        return None;
    }
    let text_a: Vec<&str> = a.iter().map(|r| &old[r.clone()]).collect();
    let text_b: Vec<&str> = b.iter().map(|r| &new[r.clone()]).collect();
    // lcs[i][j]: the longest common subsequence of a[i..] and b[j..], in tokens
    let width = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if text_a[i] == text_b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut in_a, mut in_b) = (vec![false; a.len()], vec![false; b.len()]);
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if text_a[i] == text_b[j] {
            in_a[i] = true;
            in_b[j] = true;
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let common: usize = a
        .iter()
        .zip(&in_a)
        .filter(|(_, common)| **common)
        .map(|(r, _)| r.len())
        .sum();
    let total = old.len() + new.len();
    let distance = if total == 0 {
        0.0
    } else {
        // Line lengths are far below 2^52
        #[allow(clippy::cast_precision_loss)]
        let ratio = (2 * common) as f64 / total as f64;
        1.0 - ratio
    };
    Some(WordDiff {
        old: changed_ranges(&a, &in_a),
        new: changed_ranges(&b, &in_b),
        distance,
    })
}

/// The tokens not in common, with adjacent ones merged
fn changed_ranges(tokens: &[Range<usize>], common: &[bool]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (token, &common) in tokens.iter().zip(common) {
        if common {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.end == token.start => last.end = token.end,
            _ => ranges.push(token.clone()),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_split_typst_usefully() {
        let line = "$f_(x y)$ wörds  2nd";
        let split: Vec<&str> = tokens(line).into_iter().map(|r| &line[r]).collect();
        assert_eq!(
            split,
            [
                "$", "f", "_", "(", "x", " ", "y", ")", "$", " ", "wörds", "  ", "2nd"
            ]
        );
    }

    #[test]
    fn word_diffs_find_the_changed_words() {
        let diff = word_diff("the quick fox", "the slow fox").unwrap();
        assert_eq!(diff.old, vec![4..9]);
        assert_eq!(diff.new, vec![4..8]);
        assert!(diff.distance < 0.5, "{}", diff.distance);
        let unrelated = word_diff("abc def", "xyz uvw").unwrap();
        assert!(unrelated.distance > MAX_LINE_DISTANCE);
    }

    /// Only similar lines are paired, in order, and a removed line that matches none stays
    /// unpaired.
    #[test]
    fn lines_pair_in_order_when_similar() {
        let removed = ["#cl[the quick fox][o:1]", "completely different words here"];
        let added = ["something new entirely", "#cl[the slow fox][o:1]"];
        let pairs: Vec<(usize, usize)> = pair_lines(&removed, &added)
            .into_iter()
            .map(|(r, a, _)| (r, a))
            .collect();
        assert_eq!(pairs, [(0, 1)]);
        let removed = ["a b c", "d e f"];
        let added = ["a b x", "d e y"];
        let pairs: Vec<(usize, usize)> = pair_lines(&removed, &added)
            .into_iter()
            .map(|(r, a, _)| (r, a))
            .collect();
        assert_eq!(pairs, [(0, 0), (1, 1)]);
    }

    fn hunks(old: &str, new: &str) -> String {
        source_diff_html(old, new, true, View::Hunks)
    }

    const NOTE: &str = "#se[note-id: abc]\n// spares: note start\n[#key[Green's Theorem]]\n\n#cl[the quick fox][o:1]\n\nMore text.\n// spares: note end\n\n// spares: end\n";

    #[test]
    fn snapshot_word_changed_in_cloze() {
        let new = NOTE.replace("quick", "slow");
        insta::assert_snapshot!(hunks(NOTE, &new));
    }

    #[test]
    fn snapshot_added_remark() {
        let new = NOTE.replace(
            "More text.\n",
            "More text.\n\n#remark[Only for *simple* curves.]\n",
        );
        insta::assert_snapshot!(hunks(NOTE, &new));
    }

    #[test]
    fn snapshot_changed_display_equation() {
        let old = "Green:\n$ integral.cont_C (L dif x + M dif y) = integral.double_D (M_x - L_y) dif A $\nDone.\n";
        let new = "Green:\n$ integral.cont_C (L dif x + M dif y) = integral.double_D ((partial M)/(partial x) - (partial L)/(partial y)) dif A $\nDone.\n";
        insta::assert_snapshot!(hunks(old, new));
    }

    #[test]
    fn snapshot_added_and_deleted_files() {
        let added = hunks("", "= New\n#cl[x][o:1]\n");
        let deleted = hunks("= Old\nGone\n", "");
        insta::assert_snapshot!(format!("{added}\n\n{deleted}"));
    }

    #[test]
    fn snapshot_links_only_change() {
        let old = NOTE.replace(
            "// spares: note end\n\n",
            "// spares: note end\n\nLinked Notes:\n+ x $->$ (no match found)\n\n",
        );
        let new = old.replace("(no match found)", "#link(\"/n/typst/xyz.typ\")[x]");
        insta::assert_snapshot!(hunks(&old, &new));
    }

    #[test]
    fn snapshot_escaping() {
        let old = "a < b & \"c\" 'd'\n";
        let new = "a > b && \"c\" 'd'\n";
        let html = hunks(old, new);
        assert!(!html.contains("a < b"), "{html}");
        insta::assert_snapshot!(html);
    }

    /// The full view has the whole file in one hunk; hunks have three lines of context.
    #[test]
    fn full_view_shows_every_line() {
        let old: String = (1..=20)
            .map(|i| format!("line {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let new = old.replace("line 10\n", "line ten\n");
        let full = source_diff_html(&old, &new, false, View::Full);
        assert_eq!(full.matches("class=\"hunk\"").count(), 1);
        assert_eq!(full.matches("class=\"l").count(), 21);
        let hunks = source_diff_html(&old, &new, false, View::Hunks);
        assert_eq!(hunks.matches("class=\"l").count(), 8);
        assert!(hunks.contains("@@ -7,7 +7,7 @@"), "{hunks}");
        assert!(
            hunks.contains("<span class=\"n\">10</span><span class=\"n\"></span>"),
            "{hunks}"
        );
    }

    #[test]
    fn highlights_are_flat_and_ordered() {
        let text = "*bold #f(1)*";
        let spans = highlights(text);
        for pair in spans.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start, "{spans:?}");
        }
        assert!(
            spans.iter().any(|(r, classes)| &text[r.clone()] == "f"
                && classes.contains("typ-strong")
                && classes.contains("typ-func")),
            "{spans:?}"
        );
    }
}
