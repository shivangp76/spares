use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use sqlx::SqlitePool;

use super::commit::commit_index;
use super::commit::git_index_lock;
use super::git::git;
use super::git::nul_join;
use super::note_file::construct_db_note_file;
use super::paths::note_rel_path;
use super::paths::server_repo_dir;
use crate::Error;
use crate::api::note::get_note_links_map;
use crate::api::note::get_render_note_data;
use crate::ids::migrate::IdMapping;
use crate::model::NoteId;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;

/// After `spares migrate ids` renumbered the database, moves each note's file in the notes
/// repository from its old numeric name to its letter name and rewrites it from the database (its
/// `note-id:` line changed), in one commit. Only that line changes, so git records the moves as
/// renames and `git log --follow` keeps working.
///
/// Running it again after a crash moves what is left and commits whatever is not committed yet.
/// Returns whether a commit was made.
pub async fn renumber_note_files(
    db: &SqlitePool,
    mapping: &[IdMapping],
    message: &str,
) -> Result<bool, Error> {
    let repo = server_repo_dir();
    let parsers = get_all_parsers();
    let mut parser_dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for m in mapping {
        let parser = find_parser(&m.parser_name, &parsers)?;
        parser_dirs.insert(PathBuf::from(parser.get_parser_name()));
        let old = repo.join(parser.get_parser_name()).join(format!(
            "{:0>4}.{}",
            m.old,
            parser.file_extension()
        ));
        let new = repo.join(note_rel_path(parser.as_ref(), m.new));
        if old.exists() && !new.exists() {
            std::fs::rename(&old, &new).map_err(|e| io_error(&old, e))?;
        }
    }

    let rows = get_render_note_data(db, None).await?;
    let ids: Vec<NoteId> = rows.iter().map(|row| row.note_id).collect();
    let links = if ids.is_empty() {
        HashMap::default()
    } else {
        get_note_links_map(db, &ids).await?
    };
    for row in &rows {
        let parser = find_parser(&row.parser_name, &parsers)?;
        parser_dirs.insert(PathBuf::from(parser.get_parser_name()));
        let path = repo.join(note_rel_path(parser.as_ref(), row.note_id));
        let contents = construct_db_note_file(parser.as_ref(), row, &links)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        std::fs::write(&path, contents).map_err(|e| io_error(&path, e))?;
    }

    let parser_dirs: Vec<PathBuf> = parser_dirs
        .into_iter()
        .filter(|dir| repo.join(dir).exists())
        .collect();
    let _index = git_index_lock();
    git(
        &repo,
        &[
            "add",
            "-A",
            "-f",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        Some(&nul_join(&parser_dirs)),
    )?;
    commit_index(&repo, message)
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        description: format!("Failed to access {}", path.display()),
        source,
    }
}
