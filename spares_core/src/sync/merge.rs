//! Three-way merges of a row's local and remote versions against the version they were last
//! synced at (the base). Pure functions; [`super::client`] applies the results.
//!
//! Nothing is decided by timestamps when only one side changed something: that side's version is
//! kept, however old it is. That is what keeps an old unsynced laptop edit from being overwritten
//! by a newer phone edit to a different part of the note.

use std::collections::BTreeSet;

use serde_json::Map;
use serde_json::Value;

use crate::ALLOWED_F64_ERROR;

/// Which side wins where both changed the same thing in different ways.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Local,
    Remote,
}

/// The merged value, or `None` if both sides changed it in different ways.
pub fn merge3<T: PartialEq + Clone>(base: &T, local: &T, remote: &T) -> Option<T> {
    if local == remote || remote == base {
        Some(local.clone())
    } else if local == base {
        Some(remote.clone())
    } else {
        None
    }
}

/// [`merge3`], with `prefer` deciding a conflict.
pub fn merge3_or<T: PartialEq + Clone>(base: &T, local: &T, remote: &T, prefer: Side) -> T {
    merge3(base, local, remote).unwrap_or_else(|| match prefer {
        Side::Local => local.clone(),
        Side::Remote => remote.clone(),
    })
}

/// Floats round-trip through JSON snapshots, so they are compared with a tolerance.
pub fn f64_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= ALLOWED_F64_ERROR
}

/// Additions from either side are kept and removals from either side applied. Sorted.
pub fn merge_sets<T: Ord + Clone>(base: &[T], local: &[T], remote: &[T]) -> Vec<T> {
    let base: BTreeSet<&T> = base.iter().collect();
    let local: BTreeSet<&T> = local.iter().collect();
    let remote: BTreeSet<&T> = remote.iter().collect();
    let removed: BTreeSet<&T> = base
        .iter()
        .filter(|x| !local.contains(*x) || !remote.contains(*x))
        .copied()
        .collect();
    base.iter()
        .chain(local.iter())
        .chain(remote.iter())
        .filter(|x| !removed.contains(*x))
        .map(|x| (*x).clone())
        .collect::<BTreeSet<T>>()
        .into_iter()
        .collect()
}

/// Merges JSON objects key by key (a missing key counts as a value). Anything that is not an
/// object is merged as one value. Returns the result and whether any key conflicted, in which
/// case `prefer`'s value was taken for it.
pub fn merge_json(base: &Value, local: &Value, remote: &Value, prefer: Side) -> (Value, bool) {
    let (Value::Object(b), Value::Object(l), Value::Object(r)) = (base, local, remote) else {
        let conflict = merge3(base, local, remote).is_none();
        return (merge3_or(base, local, remote, prefer), conflict);
    };
    let keys: BTreeSet<&String> = b.keys().chain(l.keys()).chain(r.keys()).collect();
    let mut merged = Map::new();
    let mut conflict = false;
    for key in keys {
        let value = merge3(&b.get(key), &l.get(key), &r.get(key)).unwrap_or_else(|| {
            conflict = true;
            match prefer {
                Side::Local => l.get(key),
                Side::Remote => r.get(key),
            }
        });
        if let Some(value) = value {
            merged.insert(key.clone(), value.clone());
        }
    }
    // Keep the local key order where it can, so an unchanged object compares equal.
    let mut ordered = Map::new();
    for key in l.keys().chain(r.keys()) {
        if let Some(value) = merged.remove(key) {
            ordered.insert(key.clone(), value);
        }
    }
    (Value::Object(ordered), conflict)
}

/// The result of merging a note's text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextMerge {
    /// One side's text, unchanged: the other side did not change it, or both made the same change.
    Unchanged(Side),
    /// Both sides changed different lines, and their changes were combined.
    Merged(String),
    /// Both sides changed the same lines.
    Conflict,
}

/// A line-based three-way merge.
pub fn merge_text(base: &str, local: &str, remote: &str) -> TextMerge {
    if local == remote || remote == base {
        return TextMerge::Unchanged(Side::Local);
    }
    if local == base {
        return TextMerge::Unchanged(Side::Remote);
    }
    match diffy::merge(base, local, remote) {
        Ok(merged) => TextMerge::Merged(merged),
        Err(_) => TextMerge::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_merge3() {
        assert_eq!(merge3(&1, &1, &1), Some(1));
        assert_eq!(merge3(&1, &2, &1), Some(2));
        assert_eq!(merge3(&1, &1, &3), Some(3));
        assert_eq!(merge3(&1, &4, &4), Some(4));
        assert_eq!(merge3(&1, &2, &3), None);
        assert_eq!(merge3_or(&1, &2, &3, Side::Remote), 3);
    }

    #[test]
    fn test_merge_sets() {
        // Each side adds one and removes one.
        assert_eq!(
            merge_sets(&[1, 2, 3], &[1, 2, 4], &[2, 3, 5]),
            vec![2, 4, 5]
        );
        // Both add the same one.
        assert_eq!(merge_sets(&[], &[7], &[7]), vec![7]);
        assert_eq!(merge_sets::<i64>(&[], &[], &[]), Vec::<i64>::new());
    }

    #[test]
    fn test_merge_json() {
        let base = json!({"a": 1, "b": 1, "c": 1});
        let local = json!({"a": 2, "b": 1, "c": 1, "d": 1});
        let remote = json!({"a": 1, "b": 3});
        let (merged, conflict) = merge_json(&base, &local, &remote, Side::Local);
        assert_eq!(merged, json!({"a": 2, "b": 3, "d": 1}));
        assert!(!conflict);

        let (merged, conflict) = merge_json(
            &json!({"a": 1}),
            &json!({"a": 2}),
            &json!({"a": 3}),
            Side::Remote,
        );
        assert_eq!(merged, json!({"a": 3}));
        assert!(conflict);
    }

    #[test]
    fn test_merge_text() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        assert_eq!(
            merge_text(base, base, "one\n2\nthree\nfour\nfive\n"),
            TextMerge::Unchanged(Side::Remote)
        );
        assert_eq!(
            merge_text(base, "one\n2\nthree\nfour\nfive\n", base),
            TextMerge::Unchanged(Side::Local)
        );
        assert_eq!(
            merge_text(
                base,
                "one\n2\nthree\nfour\nfive\n",
                "one\ntwo\nthree\nfour\n5\n"
            ),
            TextMerge::Merged("one\n2\nthree\nfour\n5\n".to_string())
        );
        assert_eq!(
            merge_text(
                base,
                "one\n2\nthree\nfour\nfive\n",
                "one\nzwei\nthree\nfour\nfive\n"
            ),
            TextMerge::Conflict
        );
    }
}
