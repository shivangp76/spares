//! Keeping raw note files in a git repository that users clone, so notes can be edited, branched and
//! reviewed with git or jj, and imported into spares by pushing.
//!
//! The server copy (`<data dir>/notes-spares`) is written only by the server, which commits after
//! every change, so its `main` describes the database. Users clone it to `<data dir>/notes`. A push
//! is imported by hooks that ask the server to [`receive`] it.
//!
//! Invariants:
//! - The database is the source of truth. Whenever files may not match it, [`reconcile`]
//!   regenerates them from it and commits.
//! - A pushed note is only imported if the database still holds the version the push is based on.
//! - A push is imported completely or rolled back.
//! - Content spares did not write is saved to a quarantine directory before being overwritten.
//!
//! Without the server copy, notes are written to `<data dir>/notes` as before.

mod commit;
pub mod git;
mod init;
mod lock;
mod note_file;
mod paths;
mod receive;
mod reconcile;

pub use commit::NEEDS_RECONCILE;
pub use commit::commit_message;
pub use commit::commit_note_files;
pub use commit::note_rel_paths;
pub use init::CheckRequest;
pub use init::CheckResponse;
pub use init::GITIGNORE;
pub use init::InitResponse;
pub use init::check;
pub use init::init_server_repo;
pub use init::init_staging_dir;
pub use init::startup;
pub use init::sync_gitignore;
pub use lock::NotesGuard;
pub use lock::WRITER_WAIT;
pub use lock::exclusive_guard;
pub use lock::run_exclusive;
pub use lock::run_holding;
pub use lock::run_shared;
pub use lock::shared_guard;
pub use note_file::NoteFields;
pub use note_file::ParsedNoteFile;
pub use note_file::file_note_fields;
pub use note_file::fill_linked_notes;
pub use note_file::parse_note_file;
pub use note_file::same_note;
pub use paths::SERVER_REPO_DIR_NAME;
pub use paths::enabled;
pub use paths::note_file_name;
pub use paths::note_rel_path;
pub use paths::note_write_dir;
pub use paths::parse_note_rel_path;
pub use paths::read_render_hash;
pub use paths::remove_render_hash;
pub use paths::server_notes_dir;
pub use paths::server_repo_dir;
pub use paths::write_render_hash;
pub use receive::FinishRequest;
pub use receive::FinishResponse;
pub use receive::README;
pub use receive::ReceiveRequest;
pub use receive::ReceiveResponse;
pub use receive::StatusResponse;
pub use receive::conflict_marker_line;
pub use receive::finish;
pub use receive::receive;
pub use receive::session_file_path;
pub use receive::status;
pub use reconcile::ReconcileReport;
pub use reconcile::ReconcileScope;
pub use reconcile::quarantine_dir;
pub use reconcile::reconcile;
pub use reconcile::reconcile_after;
