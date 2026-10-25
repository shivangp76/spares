#!/bin/sh
# Prints spares' changes to the vendored sqlx crates, as a patch against the crates.io releases.
# README.md says how to re-apply it to a new sqlx version.
set -eu
vendor=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
for dir in "$vendor"/sqlx-*-*/; do
  crate=$(basename "$dir")
  name=${crate%-*}
  version=${crate##*-}
  mkdir a b
  curl -fsSL "https://static.crates.io/crates/$name/$crate.crate" | tar xz -C a
  rm -f "a/$crate/.cargo_vcs_info.json" "a/$crate/.cargo-ok" "a/$crate/Cargo.toml.orig" "a/$crate/Cargo.lock"
  cp -R "$dir" "b/$crate"
  diff -ruN "a/$crate" "b/$crate" || true
  rm -rf a b
done
