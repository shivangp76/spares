//! How a host renders notes, set from the `[render]` table of its `host.toml` ([`RenderConfig`]).
//!
//! A render reads its collection's own directory (see [`crate::collection`]) and the
//! [`RenderConfig::read_dirs`] the host trusts every render with, and nothing else of the host's.

use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::Deserialize;
use serde::Serialize;

/// `[render]` in `host.toml`.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderConfig {
    /// Directories outside users' collections that every render may read, such as the laptop's
    /// own preambles that its templates import (`/Users/me/learning/preambles`). Empty on a host
    /// with users who don't trust each other.
    pub read_dirs: Vec<PathBuf>,
}

static CONFIG: RwLock<Option<RenderConfig>> = RwLock::new(None);

/// Sets the host's render settings, once at startup.
pub fn set_config(config: RenderConfig) {
    *CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
}

pub fn config() -> RenderConfig {
    CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

/// What a render of the current collection may read: its directories and the host's
/// [`RenderConfig::read_dirs`].
pub fn readable_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::collection::paths().enclosing_dir()];
    dirs.extend(config().read_dirs);
    dirs
}

/// The deepest directory containing every one of `dirs`, e.g. Typst's `--root`.
pub fn common_ancestor(dirs: &[PathBuf]) -> PathBuf {
    let Some((first, rest)) = dirs.split_first() else {
        return PathBuf::from("/");
    };
    let mut ancestor = first.clone();
    for dir in rest {
        while !dir.starts_with(&ancestor) {
            if !ancestor.pop() {
                return PathBuf::from("/");
            }
        }
    }
    if ancestor.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        ancestor
    }
}

/// Whether `path` is in one of [`readable_dirs`].
pub fn is_readable(path: &Path) -> bool {
    readable_dirs().iter().any(|dir| path.starts_with(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_ancestors() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(common_ancestor(&[p("/a/b/c")]), p("/a/b/c"));
        assert_eq!(common_ancestor(&[p("/a/b/c"), p("/a/b/d/e")]), p("/a/b"));
        assert_eq!(common_ancestor(&[p("/a/b"), p("/c")]), p("/"));
        assert_eq!(common_ancestor(&[]), p("/"));
    }
}
