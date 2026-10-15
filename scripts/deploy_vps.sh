#!/usr/bin/env bash
# Deploys spares to a VPS as the two-way sync server, and sets up this machine (the laptop, replica
# 0) to sync with it. Run it on the laptop, from anywhere in the repository.
#
#   scripts/deploy_vps.sh init <server> [--user <name>] [--domain <domain>] [--no-latex]
#   scripts/deploy_vps.sh update
#   scripts/deploy_vps.sh cloud-init [<name>]
#
# spares is built here, from this checkout as it is, and uploaded: the server needs neither the
# memory and disk to build it nor access to GitHub (which IPv6-only servers lack). Building needs
# zig and cargo-zigbuild (`brew install zig && cargo install --locked cargo-zigbuild`) and bun.
#
# init  First deployment, to a fresh Debian or Ubuntu VPS that `ssh root@<server>` reaches: its IP
#       with your SSH key, or its Tailscale name if it joined your tailnet when created (needed
#       when this machine cannot reach its address, e.g. an IPv6-only server). It can be re-run,
#       and asks before replacing the server's database or closing its ports:
#         1. backs up the database;
#         2. builds spares for the server and runs setup_vps.sh there as root, which creates the
#            user, asks for its sudo password and, if needed, prints a link to join your tailnet;
#         3. stores the server's API key in the Keychain, and sets `remote_host`, `url` and
#            `interval_minutes` in the [sync] table of config.toml;
#         4. has you restart the local server, uploads the database (`spares sync cloud
#            --full-upload`) and checks that both sides have the same notes;
#         5. closes the server's public ports, leaving only Tailscale (not with --domain).
#       The server is reached over Tailscale unless --domain is given, so install Tailscale on
#       this machine and your phone, logged in to the same account, first. --no-latex skips
#       LaTeX and pandoc on the server, which only markdown and LaTeX notes need.
# cloud-init  Prints a script for the VPS provider's "cloud-init user data" (or "startup script")
#       field that joins the new server to your tailnet as <name> (default: spares), so `init`
#       can reach it as root@<name> over Tailscale SSH. Use it when this machine cannot reach the
#       server's address, e.g. an IPv6-only server and no IPv6 here.
# update  Builds this checkout and installs it on the server (with sudo there). Restart the local
#       server at the same time: the two refuse to sync across database versions.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="${SPARES_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/spares}"
CONFIG_FILE="$CONFIG_DIR/config.toml"
DATA_DIR="${SPARES_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/spares}"
DB="$DATA_DIR/spares-main.sqlite"
# Where the local server's own API key is kept, if it has one. Set SPARES_API_KEY to skip the lookup.
LOCAL_KEYCHAIN_SERVICE="${LOCAL_KEYCHAIN_SERVICE:-spares-api-key}"
LOCAL_ADDRESS="${SPARES_SOCKET_ADDRESS:-127.0.0.1:8080}"
LOCAL_ADDRESS="${LOCAL_ADDRESS/0.0.0.0/127.0.0.1}"
# The Keychain item holding the server's API key
KEYCHAIN_SERVICE="spares-sync"
SHELL_FILE="$CONFIG_DIR/spares.zsh"

bold() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33m%s\033[0m\n' "$*" >&2; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }
confirm() {
  local answer
  read -r -p "$1 [Y/n] " answer
  [[ -z "$answer" || "$answer" == [Yy]* ]]
}
spares() { (cd "$REPO" && RUSTFLAGS=-Awarnings cargo run --quiet --bin spares -- "$@"); }

# Passes its input through to the terminal, with a progress bar pinned to the last line: `cmd |
# status_bar <step>...`. The bar starts at the first step and moves to the step whose title starts
# the latest "==> " line of the input, so steps that never come are skipped. It reads and writes as
# the data comes, not by lines, so prompts without a newline (e.g. sudo's) still show.
status_bar() {
  if [[ ! -t 1 ]]; then cat; return; fi
  # shellcheck disable=SC2016 # perl, not shell
  perl -e '
    use strict; use warnings;
    my @steps = @ARGV;
    my ($rows, $cols, $step, $line, $resized) = (24, 80, 0, "", 0);
    my $start = time;
    sub out { syswrite STDOUT, $_[0] }
    sub size {
      my $size = `stty size </dev/tty 2>/dev/null`;
      ($rows, $cols) = ($1, $2) if $size =~ /(\d+) (\d+)/;
    }
    # Scrolls the output above the last line, keeping the cursor where it is
    sub region { out("\e7\e[1;" . ($rows - 1) . "r\e8") }
    sub draw {
      my $elapsed = time - $start;
      my $width = 20;
      my $filled = int($width * $step / @steps);
      my $label = sprintf " %d/%d %s  %d:%02d", $step + 1, scalar @steps, $steps[$step],
        $elapsed / 60, $elapsed % 60;
      $label = substr($label, 0, $cols - $width - 1) if length($label) > $cols - $width - 1;
      out("\e7\e[$rows;1H\e[2K\e[1m" . ("\xe2\x96\x88" x $filled) . "\e[2m"
        . ("\xe2\x96\x91" x ($width - $filled)) . "\e[0m$label\e8");
    }
    my $cleaned = 0;
    sub cleanup {
      return if $cleaned++;
      out("\e7\e[r\e8\e7\e[$rows;1H\e[2K\e8");
    }
    $SIG{WINCH} = sub { $resized = 1 };
    $SIG{$_} = sub { cleanup(); exit 130 } for qw(INT TERM HUP);
    size();
    # Makes room for the bar if the cursor is on the last line
    out("\n\e[1A");
    region();
    draw();
    my $in = "";
    vec($in, fileno(STDIN), 1) = 1;
    while (1) {
      if ($resized) { $resized = 0; size(); region(); draw(); }
      my $ready = select(my $r = $in, undef, undef, 1);
      if ($ready < 0) { next if $!{EINTR}; last; }
      if ($ready == 0) { draw(); next; }
      my $n = sysread STDIN, my $data, 65536;
      next if !defined $n && $!{EINTR};
      last if !$n;
      out($data);
      $line .= $data;
      my @lines = split /\n/, $line, -1;
      $line = substr(pop @lines, -1000);
      for (@lines) {
        s/\r//g;
        next unless /^==> (.*)/;
        my $title = $1;
        for my $i ($step .. $#steps) {
          if (index($title, $steps[$i]) == 0) { $step = $i; draw(); last; }
        }
      }
    }
    cleanup();
  ' "$@"
}

# Matches the header of the [sync] table of config.toml. Passed to awk through the environment,
# since `awk -v` would interpret its backslashes.
SYNC_HEADER='^[[:space:]]*\[sync\][[:space:]]*(#.*)?$'

# This machine's time zone, e.g. America/New_York, which the server is set to so that both count
# days the same. Nothing if it cannot be told.
local_timezone() {
  local zone
  zone="$(readlink /etc/localtime 2>/dev/null)" || return 0
  [[ "$zone" == */zoneinfo/* ]] && echo "${zone#*/zoneinfo/}"
  return 0
}

# A string key of the [sync] table of config.toml, or nothing.
config_value() {
  [[ -f "$CONFIG_FILE" ]] || return 0
  SYNC_HEADER="$SYNC_HEADER" awk -v key="$1" '
    /^[[:space:]]*\[/ { in_sync = ($0 ~ ENVIRON["SYNC_HEADER"]); next }
    in_sync && $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      sub(/^[^=]*=[[:space:]]*/, ""); sub(/[[:space:]]*(#.*)?$/, ""); gsub(/^"|"$/, ""); print; exit
    }' "$CONFIG_FILE"
}

# Sets keys of the [sync] table of config.toml, given as `key = value` lines (values in TOML
# syntax). They go first in the table, which is added at the end of the file if it is missing.
set_config() {
  local lines="$1" keys tmp
  keys="$(printf '%s\n' "$lines" | sed -E 's/[[:space:]]*=.*//' | paste -sd '|' -)"
  tmp="$(mktemp)"
  if [[ -f "$CONFIG_FILE" ]] && grep -Eq "$SYNC_HEADER" "$CONFIG_FILE"; then
    # Through the environment, since `awk -v` would interpret the backslashes
    SYNC_HEADER="$SYNC_HEADER" SYNC_LINES="$lines" awk -v keys="$keys" '
      /^[[:space:]]*\[/ {
        in_sync = ($0 ~ ENVIRON["SYNC_HEADER"])
        print
        if (in_sync) print ENVIRON["SYNC_LINES"]
        next
      }
      in_sync && $0 ~ "^[[:space:]]*(" keys ")[[:space:]]*=" { next }
      { print }' "$CONFIG_FILE" > "$tmp"
  else
    {
      [[ -s "$CONFIG_FILE" ]] && { cat "$CONFIG_FILE"; echo; }
      printf '[sync]\n%s\n' "$lines"
    } > "$tmp"
  fi
  mkdir -p "$CONFIG_DIR"
  if [[ -f "$CONFIG_FILE" ]]; then
    cp -p "$CONFIG_FILE" "$CONFIG_FILE.bak"
    trap remove_unchanged_backup EXIT
  fi
  mv "$tmp" "$CONFIG_FILE"
}

# Removes config.toml.bak on exit if set_config left the file as it was.
remove_unchanged_backup() {
  cmp -s "$CONFIG_FILE" "$CONFIG_FILE.bak" && rm -f "$CONFIG_FILE.bak"
  return 0
}

# A field of the local server's `GET /api/sync/status`, or nothing if it is not running.
local_status() {
  local auth=()
  [[ -n "${SPARES_API_KEY:-}" ]] && auth=(-H "Authorization: Bearer $SPARES_API_KEY")
  curl -fsS ${auth[@]+"${auth[@]}"} "http://$LOCAL_ADDRESS/api/sync/status" 2>/dev/null \
    | plutil -extract "$1" raw -o - - 2>/dev/null || true
}

# The HTTP status of the local server's `GET /api/sync/status` (000 if it is not running).
local_http_code() {
  local auth=()
  [[ -n "${SPARES_API_KEY:-}" ]] && auth=(-H "Authorization: Bearer $SPARES_API_KEY")
  curl -s -o /dev/null -w '%{http_code}' ${auth[@]+"${auth[@]}"} "http://$LOCAL_ADDRESS/api/sync/status" || true
}

# One SSH connection per host for the whole run, so Tailscale SSH asks to log in at most once.
SSH_OPTS=(-o ControlMaster=auto -o "ControlPath=$HOME/.ssh/spares-deploy-%C" -o ControlPersist=10m)
# shellcheck disable=SC2029 # commands are built here to run there
remote() { ssh "${SSH_OPTS[@]}" "$@"; }
upload() { scp -q "${SSH_OPTS[@]}" "$@"; }

# Builds spares for the server (`$1`, as user@host) into the archive `$2`, which setup_vps.sh
# installs with --bundle: spares_server, frontend/, and with `$3` = true, pandoc.deb.
build_bundle() {
  local server="$1" bundle="$2" with_pandoc="$3" arch target deb_arch dir
  for command in zig cargo-zigbuild bun rustup; do
    command -v "$command" >/dev/null \
      || die "$command is needed to build spares for the server. Install zig and cargo-zigbuild with: brew install zig && cargo install --locked cargo-zigbuild"
  done
  arch="$(remote "$server" uname -m)"
  case "$arch" in
    x86_64) target=x86_64-unknown-linux-gnu; deb_arch=amd64 ;;
    aarch64) target=aarch64-unknown-linux-gnu; deb_arch=arm64 ;;
    *) die "Unsupported server architecture: $arch" ;;
  esac
  bold "Building spares for $target"
  rustup target add "$target" >/dev/null
  # glibc 2.31 (Debian 11, Ubuntu 20.04) or newer runs it
  (cd "$REPO" && CARGO_PROFILE_RELEASE_STRIP=debuginfo \
    cargo zigbuild --release -p spares_server --target "$target.2.31")
  (cd "$REPO/spares_frontend/frontend" && bun install && bun run build)
  dir="$(mktemp -d)"
  cp "$REPO/target/$target/release/spares_server" "$dir/"
  cp -R "$REPO/spares_frontend/frontend/dist" "$dir/frontend"
  if [[ "$with_pandoc" == true ]]; then
    local version cached
    version="$(sed -n 's/^PANDOC_VERSION="\(.*\)"/\1/p' "$REPO/scripts/setup_vps.sh")"
    cached="${XDG_CACHE_HOME:-$HOME/.cache}/spares-deploy/pandoc-$version-$deb_arch.deb"
    if [[ ! -f "$cached" ]]; then
      mkdir -p "$(dirname "$cached")"
      curl -fsSL -o "$cached.tmp" \
        "https://github.com/jgm/pandoc/releases/download/$version/pandoc-$version-1-$deb_arch.deb"
      mv "$cached.tmp" "$cached"
    fi
    cp "$cached" "$dir/pandoc.deb"
  fi
  # Without macOS metadata, which GNU tar on the server warns about
  COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -czf "$bundle" -C "$dir" .
  rm -rf "$dir"
  echo "Built $(du -h "$bundle" | cut -f1) to upload."
}

note_counts() {
  echo "SELECT (SELECT COUNT(*) FROM note) || ' notes, ' || (SELECT COUNT(*) FROM card) || ' cards, '
               || (SELECT COUNT(*) FROM review_log) || ' reviews';"
}

init() {
  local server="" user="$USER" domain="" no_latex=false timezone
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --user) user="$2"; shift 2 ;;
      --domain) domain="$2"; shift 2 ;;
      --no-latex) no_latex=true; shift ;;
      -*) die "Unknown argument: $1" ;;
      *) server="$1"; shift ;;
    esac
  done
  [[ -n "$server" ]] || die "Usage: $0 init <server> [--user <name>] [--domain <domain>] [--no-latex]"
  [[ -f "$DB" ]] || die "No database at $DB. Set SPARES_DATA_DIR if it is elsewhere."
  for command in ssh scp rsync sqlite3 curl cargo security plutil; do
    command -v "$command" >/dev/null || die "$command is needed."
  done
  if [[ -z "$domain" ]] && ! command -v tailscale >/dev/null && [[ ! -d /Applications/Tailscale.app ]]; then
    warn "Tailscale is not installed here. Install it (https://tailscale.com/download) and log in, as this machine reaches the server over it."
    confirm "Continue anyway?" || exit 1
  fi

  bold "Backing up the database"
  local backup
  backup="$DATA_DIR/spares-main.before-deploy-$(date +%Y%m%d-%H%M%S).sqlite"
  sqlite3 "$DB" "VACUUM INTO '$backup'"
  echo "Saved $backup"

  local bundle
  bundle="$(mktemp -d)/spares-bundle.tar.gz"
  build_bundle "root@$server" "$bundle" "$([[ "$no_latex" == true ]] && echo false || echo true)"

  bold "Setting up the server (as root@$server)"
  [[ "$no_latex" == false ]] && echo "Installing LaTeX takes a while."
  [[ -n "$domain" ]] && echo "Point $domain's DNS at the server first."
  [[ -z "$domain" ]] && echo "If it prints a Tailscale link, open it to add the server to your tailnet."
  local args=(--user "$user" --data-dir "$DATA_DIR" --local-home "$HOME" --bundle /tmp/spares-bundle.tar.gz)
  if [[ -n "$domain" ]]; then args+=(--domain "$domain"); else args+=(--tailscale); fi
  [[ "$no_latex" == true ]] && args+=(--no-latex)
  timezone="$(local_timezone)"
  if [[ -n "$timezone" ]]; then args+=(--timezone "$timezone"); else warn "Could not tell this machine's time zone, so the server keeps its own."; fi
  upload "$REPO/scripts/setup_vps.sh" "$bundle" "root@$server:/tmp/"
  rm -rf "$(dirname "$bundle")"
  remote -t "root@$server" bash /tmp/setup_vps.sh "${args[@]}" \
    "; status=\$?; rm -f /tmp/setup_vps.sh /tmp/spares-bundle.tar.gz; exit \$status"

  local url api_key host
  url="$(remote "root@$server" cat /etc/spares/url)"
  api_key="$(remote "root@$server" "sed -n 's/^SPARES_API_KEY=//p' /etc/spares/spares.env")"
  [[ "$url" == https://* && -n "$api_key" ]] || die "Could not read the server's address and API key."
  host="${url#https://}"
  echo "The server is at $url"

  bold "Setting up this machine"
  security add-generic-password -U -s "$KEYCHAIN_SERVICE" -a "$USER" -w "$api_key"
  echo "Stored the server's API key in the Keychain (service \"$KEYCHAIN_SERVICE\")."
  local interval answer
  interval="$(config_value interval_minutes)"
  read -r -p "Sync in the background every how many minutes (0 for never)? [${interval:-15}] " answer
  interval="${answer:-${interval:-15}}"
  [[ "$interval" =~ ^[0-9]+$ ]] || die "Not a number: $interval"
  set_config "remote_host = \"$user@$host\"
url = \"$url\"
interval_minutes = $interval"
  echo "Set remote_host, url and interval_minutes in the [sync] table of $CONFIG_FILE."
  [[ -f "$CONFIG_FILE.bak" ]] && ! cmp -s "$CONFIG_FILE" "$CONFIG_FILE.bak" \
    && echo "The old one is config.toml.bak."

  local missing=()
  grep -qs 'SPARES_REPLICA=' "$SHELL_FILE" || missing+=('export SPARES_REPLICA=0')
  grep -qs 'SPARES_SYNC_ROLE=' "$SHELL_FILE" || missing+=('export SPARES_SYNC_ROLE=client')
  grep -qs 'SPARES_SYNC_API_KEY=' "$SHELL_FILE" \
    || missing+=("export SPARES_SYNC_API_KEY=\"\$(security find-generic-password -s $KEYCHAIN_SERVICE -w 2>/dev/null)\"")
  if [[ ${#missing[@]} -gt 0 ]]; then
    echo "The local server needs these where it starts:"
    printf '  %s\n' "${missing[@]}"
    if confirm "Add them to $SHELL_FILE?"; then
      {
        echo
        echo "# Two-way sync with the server ($url). This machine is replica 0; the key is in the Keychain."
        printf '%s\n' "${missing[@]}"
      } >> "$SHELL_FILE"
    else
      echo "Add them yourself, then continue."
    fi
  fi

  bold "Restarting the local server"
  echo "Restart spares_serve in a new terminal, so it picks up the new settings."
  local tried_keychain=false wait=true pasted
  while true; do
    [[ "$wait" == true ]] && read -r -p "Press Enter once it is running. " _
    wait=true
    local role sync_url key_set
    role="$(local_status role)"
    sync_url="$(local_status sync_url)"
    key_set="$(local_status sync_api_key_set)"
    if [[ -z "$role" && "$(local_http_code)" == 401 ]]; then
      if [[ "$tried_keychain" == false ]] \
        && SPARES_API_KEY="$(security find-generic-password -a "$USER" -s "$LOCAL_KEYCHAIN_SERVICE" -w 2>/dev/null)"; then
        tried_keychain=true
        echo "Using the local server's API key from the Keychain (service \"$LOCAL_KEYCHAIN_SERVICE\")."
      else
        tried_keychain=true
        warn "The local server wants its API key (its SPARES_API_KEY)."
        read -r -s -p "Paste it here: " pasted
        echo
        # Drops what terminals wrap pasted text in, and stray whitespace.
        pasted="${pasted//$'\e[200~'/}"
        pasted="${pasted//$'\e[201~'/}"
        SPARES_API_KEY="$(printf '%s' "$pasted" | tr -d '[:space:]')"
      fi
      # Exported for the `spares` commands below, which send it too.
      export SPARES_API_KEY
      wait=false
    elif [[ -z "$role" ]]; then
      warn "No spares server answers at $LOCAL_ADDRESS."
    elif [[ "$role" != client ]]; then
      warn "It runs as the $role, not the client. Check SPARES_SYNC_ROLE."
    elif [[ "$sync_url" != "$url" ]]; then
      warn "Its sync_url is \"$sync_url\", not $url."
    elif [[ "$key_set" != true ]]; then
      warn "It has no SPARES_SYNC_API_KEY. Start it from a new terminal."
    else
      break
    fi
  done

  bold "Uploading the database"
  remote -o StrictHostKeyChecking=accept-new "$user@$host" true
  if confirm "This replaces the server's database with this one. Upload?"; then
    spares sync cloud --full-upload
  fi

  bold "Checking"
  local here there
  here="$(sqlite3 "$DB" "$(note_counts)")"
  there="$(remote "$user@$host" sqlite3 "'$DB'" "\"$(note_counts)\"")"
  echo "Here:   $here"
  echo "Server: $there"
  [[ "$here" == "$there" ]] || warn "They differ. Run \`spares sync cloud\` and compare again."
  spares sync cloud

  if [[ -z "$domain" ]]; then
    bold "Closing the server's public ports"
    echo "You reached the server over Tailscale above. This allows only Tailscale in, so SSH to its"
    echo "public address stops working; log in with \`ssh $user@$host\` instead. If Tailscale ever"
    echo "breaks, use the VPS provider's web console, which can also reset root's password."
    if confirm "Close them?"; then
      remote "root@$server" 'command -v ufw >/dev/null || apt-get install -y -q ufw >/dev/null
        ufw default deny incoming >/dev/null && ufw default allow outgoing >/dev/null
        ufw allow in on tailscale0 >/dev/null
        ufw allow 41641/udp >/dev/null  # direct Tailscale connections
        ufw --force enable' || true
    fi
  fi

  bold "Done"
  cat <<EOF
On your phone, with Tailscale on, open $url, log in with the API key
(security find-generic-password -s $KEYCHAIN_SERVICE -w) and add it to the home screen.

  spares sync cloud              sync both ways and copy files (sync.interval_minutes = $interval
                                 syncs in the background, without copying files)
  spares sync conflicts          resolve notes tagged sync-conflict
  scripts/deploy_vps.sh update   update the server
EOF
}

cloud_init() {
  local name="${1:-spares}"
  cat >&2 <<EOF
Create an auth key at https://login.tailscale.com/admin/settings/keys (not reusable, not
ephemeral), put it in place of the placeholder, and paste this into the VPS's cloud-init user
data. Once the server is up, run: $0 init $name
If \`ssh root@$name\` is refused, allow root in the "ssh" section of your tailnet policy file.

EOF
  cat <<EOF
#!/bin/sh
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --ssh --hostname=$name --authkey=TSKEY-PLACEHOLDER
EOF
}

update() {
  local remote_host
  remote_host="$(config_value remote_host)"
  [[ -n "$remote_host" ]] || die "sync.remote_host is not set in $CONFIG_FILE. Run \`$0 init\` first."
  local no_latex bundle timezone
  no_latex="$(remote "$remote_host" "sed -n 's/^NO_LATEX=//p' /etc/spares/setup.conf")"
  bundle="$(mktemp -d)/spares-bundle.tar.gz"
  build_bundle "$remote_host" "$bundle" "$([[ "$no_latex" == true ]] && echo false || echo true)"
  bold "Updating $remote_host"
  # Kept in step with this machine's, e.g. after moving
  timezone="$(local_timezone)"
  {
    upload "$REPO/scripts/setup_vps.sh" "$bundle" "$remote_host:/tmp/"
    rm -rf "$(dirname "$bundle")"
    # shellcheck disable=SC2016 # expanded there
    remote -t "$remote_host" "sudo bash /tmp/setup_vps.sh --bundle /tmp/spares-bundle.tar.gz${timezone:+ --timezone $(printf %q "$timezone")};"'
      status=$?; rm -f /tmp/setup_vps.sh /tmp/spares-bundle.tar.gz; exit $status'
  } | status_bar "Uploading" "Setting the time zone" "Installing packages" "Installing pandoc" \
    "Adding" "Installing the bundled spares" "Configuring" "Setting up"
  echo "Restart spares_serve here too, so both sides run the same version."
}

case "${1:-}" in
  init) shift; init "$@" ;;
  update) shift; update "$@" ;;
  cloud-init) shift; cloud_init "$@" ;;
  *) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
