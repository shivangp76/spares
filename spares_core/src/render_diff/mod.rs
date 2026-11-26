//! A page showing edits to note files: for each changed note, its source diff, its old render
//! and its new render, side by side. `spares vcs render-diff` builds it from the two directories a
//! jj or git diff tool gets, with the renders from the host's `POST /api/notes/vcs/render-preview`
//! (see [`crate::render::preview_typst`]).
//!
//! The renders are shown side by side rather than merged into one marked-up render (as latexdiff
//! does): that was tried and was too much to read.
//!
//! A Typst SVG page is about 200 KB, a `<use>` per glyph with 32-hex ids, so the page holds each
//! one gzipped and inflates it only when it is shown (see [`page`]). 420 notes make a page of
//! 44 MB rather than 230 MB.

pub mod page;
pub mod pairs;
pub mod source_diff;

pub use page::NoteDiff;
pub use page::PageInfo;
pub use page::page_html;
pub use pairs::ChangeKind;
pub use pairs::NoteChange;
pub use pairs::NoteLanguage;
pub use pairs::note_changes;

/// A side of a diff
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffSide {
    Old,
    New,
}

/// The name a side of note `id` is rendered under (see [`crate::render::PreviewSource`]):
/// `<id>-old` or `<id>-new`, with any character a preview name can't have replaced by `_`
pub fn preview_name(id: &str, side: DiffSide) -> String {
    let id: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    match side {
        DiffSide::Old => format!("{id}-old"),
        DiffSide::New => format!("{id}-new"),
    }
}
