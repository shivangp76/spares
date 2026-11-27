#!/usr/bin/env bash
# Fills a new jj workspace with what the repository doesn't track but builds need, copied from
# another checkout: the svgedit submodule, node_modules, the WASM package in src/local/pkg, and
# target/debug, so the first build is incremental. Run it from anywhere in the new workspace:
#
#   scripts/setup_workspace.sh <main checkout>
#
# jj doesn't check out git submodules: it leaves an empty directory where svgedit goes. Copying
# into that directory would nest the copy as svgedit/svgedit, so an empty destination is replaced.
# A destination with something in it is left alone. svgedit can't be a symlink, which vite
# rejects. Copies are clones where the file system has them (APFS, Btrfs, XFS), so they are instant
# and take no space until changed.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <main checkout>" >&2
  exit 2
fi
from=$(cd "$1" && pwd)
to=$(cd "$(dirname "$0")/.." && pwd)
if [[ "$from" == "$to" ]]; then
  echo "$from is this checkout: give the checkout to copy from" >&2
  exit 2
fi

clone() {
  if [[ "$(uname)" == Darwin ]]; then
    cp -cR "$1" "$2"
  else
    cp -R --reflink=auto "$1" "$2"
  fi
}

copy() {
  local path=$1 source="$from/$1" dest="$to/$1"
  if [[ ! -d "$source" ]]; then
    echo "skipped $path: not in $from"
    return
  fi
  if [[ -L "$dest" ]]; then
    echo "skipped $path: it is a symlink (remove it to copy instead)" >&2
    return
  fi
  if [[ -d "$dest" ]]; then
    if [[ -n "$(ls -A "$dest")" ]]; then
      echo "skipped $path: already there"
      return
    fi
    rmdir "$dest"
  fi
  mkdir -p "$(dirname "$dest")"
  clone "$source" "$dest"
  echo "copied $path"
}

copy spares_frontend/frontend/svgedit
copy spares_frontend/frontend/node_modules
copy spares_frontend/frontend/src/local/pkg
copy target/debug

if [[ -L "$to/spares_frontend/frontend/svgedit" ]]; then
  echo "spares_frontend/frontend/svgedit is a symlink, which vite rejects" >&2
  exit 1
fi
