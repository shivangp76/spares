use std::path::Path;
use std::path::PathBuf;

use crate::config::get_cache_dir;
use crate::config::get_data_dir;
use crate::config::is_test_mode;
use crate::model::NoteId;
use crate::parsers::Parseable;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::parsers::get_output_raw_dir;

/// Name of the server copy of the notes repository, relative to the data directory.
pub const SERVER_REPO_DIR_NAME: &str = "notes-spares";

/// The server copy of the notes repository, whether or not it exists.
pub fn server_repo_dir() -> PathBuf {
    get_data_dir().join(SERVER_REPO_DIR_NAME)
}

/// Whether notes are kept in a git repository that users push to. Without it, notes are written to
/// the user-facing directory as before.
pub fn enabled() -> bool {
    !is_test_mode() && server_repo_dir().join(".git").exists()
}

/// The directory the server writes raw note files to. Readers that show paths to users use
/// [`get_output_raw_dir`] instead, which is the user's clone when VCS is enabled.
pub fn server_notes_dir() -> PathBuf {
    if enabled() {
        server_repo_dir()
    } else {
        get_data_dir().join("notes")
    }
}

/// Where raw note files of `parser_name` are written. Files requested in another directory go
/// there unchanged.
pub fn note_write_dir(parser_name: &str, overridden_base_dir: Option<&Path>) -> PathBuf {
    match overridden_base_dir {
        Some(_) => get_output_raw_dir(parser_name, RenderOutputType::Note, overridden_base_dir),
        None => server_notes_dir().join(parser_name),
    }
}

/// The only file name a note's raw file can have, e.g. `0042.typ`.
pub fn note_file_name(parser: &dyn Parseable, note_id: NoteId) -> String {
    let mut name = PathBuf::from(parser.get_output_filename(RenderOutputType::Note, note_id));
    name.set_extension(parser.file_extension());
    name.to_string_lossy().into_owned()
}

/// A note's raw file relative to the notes directory, e.g. `typst/0042.typ`.
pub fn note_rel_path(parser: &dyn Parseable, note_id: NoteId) -> PathBuf {
    Path::new(parser.get_parser_name()).join(note_file_name(parser, note_id))
}

/// The relative paths a note's raw file could have under any installed parser. A note whose parser
/// changed may have left a file under another one.
pub fn note_rel_paths_all_parsers(note_id: NoteId) -> Vec<PathBuf> {
    get_all_parsers()
        .into_iter()
        .map(|parser| note_rel_path(parser().as_ref(), note_id))
        .collect()
}

/// The parser and note id of a canonical note path relative to the notes directory, or `None` if
/// `rel_path` is not one.
pub fn parse_note_rel_path(rel_path: &Path) -> Option<(Box<dyn Parseable>, NoteId)> {
    let mut components = rel_path.components();
    let parser_name = components.next()?.as_os_str().to_str()?;
    let file_name = components.next()?.as_os_str().to_str()?;
    if components.next().is_some() {
        return None;
    }
    let parser = get_all_parsers()
        .into_iter()
        .map(|parser| parser())
        .find(|parser| parser.get_parser_name() == parser_name)?;
    let stem = Path::new(file_name).file_stem()?.to_str()?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let note_id: NoteId = stem.parse().ok()?;
    // Only the canonical name counts, so `gir.TYP` is not a note path.
    (note_file_name(parser.as_ref(), note_id) == file_name).then_some((parser, note_id))
}

/// The parser of a new note's file relative to the notes directory, `<parser>/new-<name>.<ext>`
/// (e.g. `typst/new-bayes.typ`), or `None` if `rel_path` is not one. `<name>` is lowercase ASCII
/// letters, digits and `-`, so the file name never parses as a note id, which has no `-`.
pub fn parse_new_note_rel_path(rel_path: &Path) -> Option<Box<dyn Parseable>> {
    let mut components = rel_path.components();
    let parser_name = components.next()?.as_os_str().to_str()?;
    let file_name = components.next()?.as_os_str().to_str()?;
    if components.next().is_some() {
        return None;
    }
    let parser = get_all_parsers()
        .into_iter()
        .map(|parser| parser())
        .find(|parser| parser.get_parser_name() == parser_name)?;
    let (stem, extension) = file_name.rsplit_once('.')?;
    let name = stem.strip_prefix("new-")?;
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (valid && extension == parser.file_extension()).then_some(parser)
}

/// Holds `blake3` of the raw file a note was last compiled from. In the notes repository, raw files
/// do not carry their hash (see [`crate::parsers::generate_files::file_in_cache`]).
pub fn render_hash_path(parser_name: &str, note_id: NoteId) -> PathBuf {
    get_cache_dir()
        .join("note-hashes")
        .join(parser_name)
        .join(note_id.letters())
}

pub fn read_render_hash(parser_name: &str, note_id: NoteId) -> Option<String> {
    std::fs::read_to_string(render_hash_path(parser_name, note_id))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn write_render_hash(parser_name: &str, note_id: NoteId, hash: &str) {
    let path = render_hash_path(parser_name, note_id);
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, hash));
    if let Err(e) = result {
        log::warn!("Failed to write {}: {e}", path.display());
    }
}

pub fn remove_render_hash(parser_name: &str, note_id: NoteId) {
    let _ = std::fs::remove_file(render_hash_path(parser_name, note_id));
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::parse_new_note_rel_path;
    use super::parse_note_rel_path;

    #[test]
    fn test_parse_note_rel_path() {
        let (parser, id) = parse_note_rel_path(Path::new("typst/gir.typ")).unwrap();
        assert_eq!(parser.get_parser_name(), "typst");
        assert_eq!(id, "gir".parse().unwrap());
        assert!(parse_note_rel_path(Path::new("typst/gizzaa.typ")).is_some());
        for path in [
            "typst/0042.typ",
            "typst/42.typ",
            "typst/GIR.typ",
            "typst/gir.TYP",
            "Typst/gir.typ",
            "typst/gir.md",
            "typst/sub/gir.typ",
            "typst",
            "unknown/gir.typ",
            "README.md",
            "typst/gi-r.typ",
            "typst/abcdefghi.typ",
        ] {
            assert!(parse_note_rel_path(Path::new(path)).is_none(), "{path}");
        }
    }

    #[test]
    fn test_parse_new_note_rel_path() {
        for path in [
            "typst/new-bayes.typ",
            "typst/new-anki-1712345.typ",
            "markdown/new-a-b.md",
        ] {
            assert!(parse_new_note_rel_path(Path::new(path)).is_some(), "{path}");
        }
        for path in [
            "typst/new.typ",
            "typst/new-.typ",
            "typst/new-Bayes.typ",
            "typst/new-bayes.md",
            "typst/new-b_c.typ",
            "typst/sub/new-bayes.typ",
            "unknown/new-bayes.typ",
            "typst/gir.typ",
        ] {
            assert!(parse_new_note_rel_path(Path::new(path)).is_none(), "{path}");
        }
        // A new note's name is never a note's.
        assert!(parse_note_rel_path(Path::new("typst/new-bayes.typ")).is_none());
    }
}
