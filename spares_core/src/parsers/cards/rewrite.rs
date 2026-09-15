use std::ops::Range;

/// One text replacement: replace `original[range]` with `new_text`.
pub(super) struct RangeReplacement {
    pub range: Range<usize>,
    pub new_text: String,
}

/// Rebuilds `original` by replacing each replacement's range with its `new_text`,
/// via a single left-to-right pass. Replacement ranges must be pairwise
/// non-overlapping — they may sit nested inside a larger *untouched* span (e.g. a
/// nested cloze's delimiter replacements naturally fall inside its parent's
/// untouched body gap) as long as the replacement ranges themselves never overlap
/// each other. Does not need to be pre-sorted; this function sorts by
/// `range.start`.
pub(super) fn rewrite_ranges(original: &str, mut replacements: Vec<RangeReplacement>) -> String {
    replacements.sort_unstable_by_key(|r| r.range.start);
    let mut out = String::with_capacity(original.len());
    let mut prev_end = 0;
    for r in &replacements {
        out.push_str(&original[prev_end..r.range.start]);
        out.push_str(&r.new_text);
        prev_end = r.range.end;
    }
    out.push_str(&original[prev_end..]);
    out
}
