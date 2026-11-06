//! Reusing a batch's rendered cards: a card whose source is now what another card of the batch was
//! made from is copied from that card's file instead of being rendered again.
//!
//! This is what makes inserting a note into a live file cheap. Live notes are matched by position
//! (see `api::note::import`), so the text of every note after the new one moves to the next note,
//! and so do their cards. A card's source doesn't name its note (see
//! [`cloze_tag_str`](crate::parsers::cloze_tag_str)), so its [render key](crate::parsers::generate_files::RenderState::key) moves
//! with its text, and the card's file can be copied from the file it had under the note before.
//!
//! Before a batch renders anything, [`RenderReuse::snapshot`] hard-links each rendered card the
//! batch's notes have a [`RenderState`](crate::parsers::generate_files::RenderState) for into a directory of its own, by key. While the snapshot
//! is kept, a render moves the file it replaces aside first ([`render_aside`]) instead of writing
//! over it, so the links keep the contents from before. The directory is removed when the
//! snapshot is dropped.
//!
//! Only cards are reused: a note's own file names the note. Cards with image occlusions are always
//! rendered, since their images are made beside the rendered card.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use log::debug;
use log::warn;

use crate::Error;
use crate::model::NoteId;
use crate::parsers::Parseable;
use crate::parsers::RenderDependencies;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::generate_files::RenderStates;
use crate::parsers::web_render_filepath;

/// A rendered card as it was before the batch
struct Reusable {
    rendered: PathBuf,
    /// Its HTML for web clients, if it had any
    web: Option<PathBuf>,
    dependencies: RenderDependencies,
}

/// See the [module docs](self)
pub(crate) struct RenderReuse {
    dir: PathBuf,
    by_key: HashMap<String, Reusable>,
}

impl RenderReuse {
    /// Links the rendered cards of `render_states`' notes into a new directory under the
    /// collection's render cache. `None` if there are none, or the directory can't be made.
    pub(crate) fn snapshot(
        parser: &dyn Parseable,
        render_states: &HashMap<NoteId, RenderStates>,
    ) -> Option<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        if render_states.is_empty() {
            return None;
        }
        let dir = crate::collection::paths().render.join(format!(
            "reuse-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("Failed to create {}: {e}", dir.display());
            return None;
        }
        let mut reuse = Self {
            dir,
            by_key: HashMap::new(),
        };
        let cards_dir = parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
        for (&note_id, states) in render_states {
            for (&output, state) in states {
                if matches!(output, RenderOutputType::Note) || reuse.by_key.contains_key(&state.key)
                {
                    continue;
                }
                let rendered = cards_dir.join(parser.get_output_filename(output, note_id));
                let index = reuse.by_key.len();
                let link = |from: &Path, extension: &str| {
                    let to = reuse.dir.join(format!("{index}.{extension}"));
                    std::fs::hard_link(from, &to).ok().map(|()| to)
                };
                let Some(linked) = link(&rendered, "rendered") else {
                    continue;
                };
                let web = web_render_filepath(&rendered);
                let web = if web.exists() {
                    match link(&web, "html") {
                        Some(web) => Some(web),
                        None => continue,
                    }
                } else {
                    None
                };
                reuse.by_key.insert(
                    state.key.clone(),
                    Reusable {
                        rendered: linked,
                        web,
                        dependencies: state.dependencies.clone(),
                    },
                );
            }
        }
        Some(reuse)
    }

    /// Puts the card rendered from `key` before the batch at `rendered`, with its HTML for web
    /// clients, and returns what it was rendered from. `None` if no card of the batch was
    /// rendered from `key`, or it couldn't be copied, so the card is rendered.
    pub(crate) fn copy(&self, key: &str, rendered: &Path) -> Option<RenderDependencies> {
        let reusable = self.by_key.get(key)?;
        let web = web_render_filepath(rendered);
        let result =
            copy_into_place(&reusable.rendered, rendered).and_then(|()| match &reusable.web {
                Some(from) => copy_into_place(from, &web),
                None => match std::fs::remove_file(&web) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                },
            });
        match result {
            Ok(()) => {
                debug!("Copied {} from an earlier render", rendered.display());
                Some(reusable.dependencies.clone())
            }
            Err(e) => {
                warn!(
                    "Failed to copy {} from an earlier render, rendering it: {e}",
                    rendered.display()
                );
                None
            }
        }
    }
}

impl Drop for RenderReuse {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            warn!("Failed to remove {}: {e}", self.dir.display());
        }
    }
}

/// Copies `from` to `to` through a file beside `to`, so `to` is replaced, never written over. The
/// copy is modified now, as a render would leave it, since what syncs rendered files looks at
/// when they were modified (a copy on macOS keeps the original's time).
fn copy_into_place(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = with_suffix(to, ".reuse.tmp");
    let copied = std::fs::copy(from, &tmp).and_then(|_| {
        std::fs::File::options()
            .write(true)
            .open(&tmp)?
            .set_modified(std::time::SystemTime::now())?;
        std::fs::rename(&tmp, to)
    });
    copied.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Runs `render`, which writes `rendered`, with the file there before moved aside, so that a
/// [`RenderReuse`] snapshot linking to it keeps its contents. It is put back if `render` fails or
/// writes nothing, as if `render` had left it.
pub(crate) fn render_aside<T>(
    rendered: &Path,
    render: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let aside = with_suffix(rendered, ".before");
    let moved = std::fs::rename(rendered, &aside).is_ok();
    let result = render();
    if moved {
        if result.is_ok() && rendered.exists() {
            let _ = std::fs::remove_file(&aside);
        } else {
            let _ = std::fs::rename(&aside, rendered);
        }
    }
    result
}
