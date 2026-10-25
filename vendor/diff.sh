#!/bin/sh
# Prints spares' changes to the vendored sqlx crates, as a patch against the crates.io releases.
# README.md says how to re-apply it to a new sqlx version.
#
# The vendored copies are formatted with spares' rustfmt.toml, so the releases are formatted the
# same way first (needs nightly rustfmt), and the patch has only the real changes.
set -eu
vendor=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
for dir in "$vendor"/sqlx-*-*/; do
  crate=$(basename "$dir")
  name=${crate%-*}
  mkdir a b
  curl -fsSL "https://static.crates.io/crates/$name/$crate.crate" | tar xz -C a
  rm -f "a/$crate/.cargo_vcs_info.json" "a/$crate/.cargo-ok" "a/$crate/Cargo.toml.orig" "a/$crate/Cargo.lock"
  find "a/$crate" -name '*.rs' -exec rustfmt +nightly --edition 2024 --config-path "$vendor/../rustfmt.toml" {} +
  # Without a `target` left by building inside the vendored crate
  tar -C "$vendor" --exclude "$crate/target" -cf - "$crate" | tar -xf - -C b
  diff -ruN "a/$crate" "b/$crate" || true
  rm -rf a b
done
