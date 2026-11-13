//! Pairing the files of a diff's two sides, and telling what changed in each note.

use std::path::Path;

use crate::parsers::Parseable;
use crate::parsers::impls::typst::TypstParser;

/// How a note file changed between the two sides
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ChangeKind {
    Modified,
    /// Only on the new side
    Added,
    /// Only on the old side
    Deleted,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
        }
    }
}

/// The language of a note file, by its extension
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteLanguage {
    Typst,
    Markdown,
}

impl NoteLanguage {
    pub fn of_path(path: &str) -> Option<Self> {
        match Path::new(path).extension()?.to_str()? {
            "typ" => Some(Self::Typst),
            "md" => Some(Self::Markdown),
            _ => None,
        }
    }
}

/// A note file that differs between the two sides of a diff
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteChange {
    /// Its path relative to the sides' directories, with `/` between components
    pub path: String,
    /// The note's id: the file's stem
    pub id: String,
    /// The text of its first `#key[...]`, on the new side, else the old
    pub title: Option<String>,
    pub kind: ChangeKind,
    pub language: NoteLanguage,
    /// A modified note whose only changed lines are in the `Linked Notes:` list spares writes
    /// after the note
    pub links_only: bool,
    /// Its text on the old side, `None` if added
    pub old: Option<String>,
    /// Its text on the new side, `None` if deleted
    pub new: Option<String>,
}

/// The note files (`.typ` and `.md`) that differ between the directories `left` (old) and
/// `right` (new), as a diff tool such as jj's gets them, sorted by path. Other files, and files
/// that aren't UTF-8, are left out.
pub fn note_changes(left: &Path, right: &Path) -> std::io::Result<Vec<NoteChange>> {
    let mut paths = std::collections::BTreeSet::new();
    for dir in [left, right] {
        if dir.exists() {
            collect_files(dir, "", &mut paths)?;
        }
    }
    let read = |dir: &Path, path: &str| std::fs::read_to_string(dir.join(path)).ok();
    Ok(paths
        .into_iter()
        .filter_map(|path| {
            let language = NoteLanguage::of_path(&path)?;
            let (old, new) = (read(left, &path), read(right, &path));
            note_change(path, language, old, new)
        })
        .collect())
}

/// The change from `old` to `new` of the note file at `path`. `None` if it has neither side, or
/// both and they are the same.
pub fn note_change(
    path: String,
    language: NoteLanguage,
    old: Option<String>,
    new: Option<String>,
) -> Option<NoteChange> {
    let kind = match (&old, &new) {
        (Some(old), Some(new)) if old == new => return None,
        (Some(_), Some(_)) => ChangeKind::Modified,
        (None, Some(_)) => ChangeKind::Added,
        (Some(_), None) => ChangeKind::Deleted,
        (None, None) => return None,
    };
    let id = Path::new(&path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let title = match language {
        NoteLanguage::Typst => new
            .as_deref()
            .and_then(first_keyword)
            .or_else(|| old.as_deref().and_then(first_keyword)),
        NoteLanguage::Markdown => None,
    };
    let links_only = match (&old, &new) {
        (Some(old), Some(new)) => without_linked_notes(old) == without_linked_notes(new),
        _ => false,
    };
    Some(NoteChange {
        path,
        id,
        title,
        kind,
        language,
        links_only,
        old,
        new,
    })
}

fn collect_files(
    dir: &Path,
    prefix: &str,
    paths: &mut std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        // Follows links, as jj may link files into its directories
        let metadata = std::fs::metadata(entry.path())?;
        if metadata.is_dir() {
            collect_files(&entry.path(), &path, paths)?;
        } else if metadata.is_file() {
            paths.insert(path);
        }
    }
    Ok(())
}

/// The text of the first `#key[...]` of a Typst note, through the parser's keyword extraction
fn first_keyword(text: &str) -> Option<String> {
    let ranges = TypstParser::new().get_embedded_keywords(text).ok()?;
    let keyword = text.get(ranges.first()?.clone())?.trim();
    (!keyword.is_empty()).then(|| keyword.to_string())
}

/// The heading of the list of linked notes that spares writes after a note
const LINKED_NOTES_HEADING: &str = "Linked Notes:";

/// `text` without the `Linked Notes:` list spares writes after a note, from its heading to the
/// file's `spares: end` comment (or its end), and without the blank lines before where it was, so
/// that a note with the list and one without compare equal.
fn without_linked_notes(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let heading = lines
        .iter()
        .rposition(|line| line.trim_end() == LINKED_NOTES_HEADING);
    let end_marker = |from: usize| {
        lines[from..]
            .iter()
            .position(|line| line.contains("spares: end"))
            .map(|i| from + i)
    };
    let (start, end) = match heading {
        Some(heading) => (heading, end_marker(heading).unwrap_or(lines.len())),
        None => match end_marker(0) {
            Some(end) => (end, end),
            None => return text.to_string(),
        },
    };
    let mut start = start;
    while start > 0 && lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    lines[..start]
        .iter()
        .chain(&lines[end..])
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(body: &str, links: &str) -> String {
        let links = if links.is_empty() {
            String::new()
        } else {
            format!("\nLinked Notes:\n{links}\n")
        };
        format!(
            "#se[note-id: abc]\n// spares: note start\n[#key[ Green's Theorem ]]\n{body}\n// spares: note \
             end\n{links}\n// spares: end\n"
        )
    }

    #[test]
    fn pairs_files_of_both_sides() {
        let root =
            std::env::temp_dir().join(format!("spares-render-diff-pairs-{}", std::process::id()));
        let (left, right) = (root.join("left"), root.join("right"));
        for dir in [&left, &right] {
            std::fs::create_dir_all(dir.join("typst")).unwrap();
        }
        std::fs::write(left.join("typst/mod.typ"), note("old", "")).unwrap();
        std::fs::write(right.join("typst/mod.typ"), note("new", "")).unwrap();
        std::fs::write(left.join("typst/del.typ"), note("gone", "")).unwrap();
        std::fs::write(right.join("typst/add.typ"), note("here", "")).unwrap();
        std::fs::write(left.join("typst/same.typ"), "same").unwrap();
        std::fs::write(right.join("typst/same.typ"), "same").unwrap();
        std::fs::write(right.join("notes.txt"), "not a note").unwrap();
        std::fs::create_dir_all(right.join("markdown")).unwrap();
        std::fs::write(right.join("markdown/md.md"), "# A note").unwrap();

        let changes = note_changes(&left, &right).unwrap();
        let summary: Vec<(&str, &str, ChangeKind)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.id.as_str(), c.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("markdown/md.md", "md", ChangeKind::Added),
                ("typst/add.typ", "add", ChangeKind::Added),
                ("typst/del.typ", "del", ChangeKind::Deleted),
                ("typst/mod.typ", "mod", ChangeKind::Modified),
            ]
        );
        assert_eq!(changes[0].language, NoteLanguage::Markdown);
        assert_eq!(changes[0].title, None);
        assert_eq!(changes[3].title.as_deref(), Some("Green's Theorem"));
        assert_eq!(changes[2].new, None);
        assert_eq!(changes[1].old, None);
        // A side may be missing altogether, as for a diff from the root commit
        let added = note_changes(&root.join("none"), &right).unwrap();
        assert!(added.iter().all(|c| c.kind == ChangeKind::Added));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tells_changes_to_linked_notes_only() {
        let change = |old: String, new: String| {
            note_change(
                "typst/abc.typ".to_string(),
                NoteLanguage::Typst,
                Some(old),
                Some(new),
            )
            .unwrap()
        };
        let a = "+ x $->$ (no match found)";
        let b = "+ x $->$ #link(\"/n/abc.typ\")[x]";
        assert!(change(note("body", a), note("body", b)).links_only);
        assert!(change(note("body", ""), note("body", b)).links_only);
        assert!(change(note("body", a), note("body", "")).links_only);
        assert!(!change(note("body", a), note("edited", b)).links_only);
        assert!(!change(note("body", ""), note("edited", "")).links_only);
    }
}
