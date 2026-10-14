#!/usr/bin/env bash
# Deploys spares to a VPS as the two-way sync server, and sets up this machine (the laptop, replica
# 0) to sync with it. Run it on the laptop, from anywhere in the repository.
#
#   scripts/deploy_vps.sh init <server IP> [--user <name>] [--domain <domain>]
#   scripts/deploy_vps.sh update
#
# init  First deployment, to a fresh Debian or Ubuntu VPS that `ssh root@<server IP>` reaches with
#       your key. It can be re-run, and asks before replacing the server's database or closing
#       its ports:
#         1. checks that GitHub's main has this checkout, since the server builds from it;
#         2. backs up the database;
#         3. runs setup_vps.sh on the server as root, which creates the user, asks for its sudo
#            password and prints a link to add the server to your tailnet;
#         4. stores the server's API key in the Keychain, and sets `remote_host`, `sync_url` and
#            `sync_interval_minutes` in config.toml;
#         5. has you restart the local server, uploads the database (`spares sync cloud
#            --full-upload`) and checks that both sides have the same notes;
#         6. closes the server's public ports, leaving only Tailscale (not with --domain).
#       The server is reached over Tailscale unless --domain is given, so install Tailscale on
#       this machine and your phone, logged in to the same account, first.
# update  Pulls and rebuilds on the server (re-runs setup_vps.sh there with sudo). Update this
#       machine's spares at the same time: the two refuse to sync across database versions.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="${SPARES_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/spares}"
CONFIG_FILE="$CONFIG_DIR/config.toml"
DATA_DIR="${SPARES_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/spares}"
DB="$DATA_DIR/spares-main.sqlite"
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

# A top-level string key of config.toml, or nothing.
config_value() {
  [[ -f "$CONFIG_FILE" ]] || return 0
  awk -v key="$1" '
    /^[[:space:]]*\[/ { exit }
    $0 ~ "^[[:space:]]*" key "[[:space:]]*=" {
      sub(/^[^=]*=[[:space:]]*/, ""); sub(/[[:space:]]*(#.*)?$/, ""); gsub(/^"|"$/, ""); print; exit
    }' "$CONFIG_FILE"
}

# Sets top-level keys of config.toml, given as `key = value` lines (values in TOML syntax). They go
# first in the file, since top-level keys after a [table] would belong to it.
set_config() {
  local lines="$1" keys tmp
  keys="$(printf '%s\n' "$lines" | sed -E 's/[[:space:]]*=.*//' | paste -sd '|' -)"
  tmp="$(mktemp)"
  {
    printf '%s\n' "$lines"
    [[ -f "$CONFIG_FILE" ]] && awk -v keys="$keys" '
      /^[[:space:]]*\[/ { in_table = 1 }
      !in_table && $0 ~ "^[[:space:]]*(" keys ")[[:space:]]*=" { next }
      { print }' "$CONFIG_FILE"
  } > "$tmp"
  mkdir -p "$CONFIG_DIR"
  [[ -f "$CONFIG_FILE" ]] && cp "$CONFIG_FILE" "$CONFIG_FILE.bak"
  mv "$tmp" "$CONFIG_FILE"
}

# A field of the local server's `GET /api/sync/status`, or nothing if it is not running.
local_status() {
  local auth=()
  [[ -n "${SPARES_API_KEY:-}" ]] && auth=(-H "Authorization: Bearer $SPARES_API_KEY")
  curl -fsS ${auth[@]+"${auth[@]}"} "http://$LOCAL_ADDRESS/api/sync/status" 2>/dev/null \
    | plutil -extract "$1" raw -o - - 2>/dev/null || true
}

# Whether GitHub's main has the commit this checkout is at, since the server builds from it.
check_pushed() {
  bold "Checking that GitHub's main has this checkout"
  if ! git -C "$REPO" fetch --quiet origin main; then
    warn "Could not fetch GitHub's main."
  elif git -C "$REPO" merge-base --is-ancestor HEAD FETCH_HEAD; then
    echo "It does."
    return
  else
    warn "GitHub's main does not have this checkout's commit, so the server would build an older spares."
  fi
  confirm "Continue anyway?" || exit 1
}

note_counts() {
  echo "SELECT (SELECT COUNT(*) FROM note) || ' notes, ' || (SELECT COUNT(*) FROM card) || ' cards, '
               || (SELECT COUNT(*) FROM review_log) || ' reviews';"
}

init() {
  local ip="" user="$USER" domain=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --user) user="$2"; shift 2 ;;
      --domain) domain="$2"; shift 2 ;;
      -*) die "Unknown argument: $1" ;;
      *) ip="$1"; shift ;;
    esac
  done
  [[ -n "$ip" ]] || die "Usage: $0 init <server IP> [--user <name>] [--domain <domain>]"
  [[ -f "$DB" ]] || die "No database at $DB. Set SPARES_DATA_DIR if it is elsewhere."
  for command in ssh scp rsync sqlite3 curl cargo security plutil; do
    command -v "$command" >/dev/null || die "$command is needed."
  done
  if [[ -z "$domain" ]] && ! command -v tailscale >/dev/null && [[ ! -d /Applications/Tailscale.app ]]; then
    warn "Tailscale is not installed here. Install it (https://tailscale.com/download) and log in, as this machine reaches the server over it."
    confirm "Continue anyway?" || exit 1
  fi

  check_pushed

  bold "Backing up the database"
  local backup
  backup="$DATA_DIR/spares-main.before-deploy-$(date +%Y%m%d-%H%M%S).sqlite"
  sqlite3 "$DB" "VACUUM INTO '$backup'"
  echo "Saved $backup"

  bold "Setting up the server (as root@$ip)"
  echo "This takes a while: it installs LaTeX and builds spares."
  [[ -n "$domain" ]] && echo "Point $domain's DNS at $ip first."
  [[ -z "$domain" ]] && echo "Open the Tailscale link it prints to add the server to your tailnet."
  local mode=(--tailscale)
  [[ -n "$domain" ]] && mode=(--domain "$domain")
  scp -q "$REPO/scripts/setup_vps.sh" "root@$ip:/tmp/setup_vps.sh"
  ssh -t "root@$ip" bash /tmp/setup_vps.sh --user "$user" --data-dir "$DATA_DIR" \
    --local-home "$HOME" "${mode[@]}"

  local url api_key host
  url="$(ssh "root@$ip" cat /etc/spares/url)"
  api_key="$(ssh "root@$ip" "sed -n 's/^SPARES_API_KEY=//p' /etc/spares/spares.env")"
  [[ "$url" == https://* && -n "$api_key" ]] || die "Could not read the server's address and API key."
  host="${url#https://}"
  echo "The server is at $url"

  bold "Setting up this machine"
  security add-generic-password -U -s "$KEYCHAIN_SERVICE" -a "$USER" -w "$api_key"
  echo "Stored the server's API key in the Keychain (service \"$KEYCHAIN_SERVICE\")."
  local interval answer
  interval="$(config_value sync_interval_minutes)"
  read -r -p "Sync in the background every how many minutes (0 for never)? [${interval:-15}] " answer
  interval="${answer:-${interval:-15}}"
  [[ "$interval" =~ ^[0-9]+$ ]] || die "Not a number: $interval"
  set_config "remote_host = \"$user@$host\"
sync_url = \"$url\"
sync_interval_minutes = $interval"
  echo "Set remote_host, sync_url and sync_interval_minutes in $CONFIG_FILE (the old one is config.toml.bak)."

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
  while true; do
    read -r -p "Press Enter once it is running. " _
    local role sync_url key_set
    role="$(local_status role)"
    sync_url="$(local_status sync_url)"
    key_set="$(local_status sync_api_key_set)"
    if [[ -z "$role" ]]; then
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
  ssh -o StrictHostKeyChecking=accept-new "$user@$host" true
  if confirm "This replaces the server's database with this one. Upload?"; then
    spares sync cloud --full-upload
  fi

  bold "Checking"
  local here there
  here="$(sqlite3 "$DB" "$(note_counts)")"
  # shellcheck disable=SC2029 # the query is built here on purpose
  there="$(ssh "$user@$host" sqlite3 "'$DB'" "\"$(note_counts)\"")"
  echo "Here:   $here"
  echo "Server: $there"
  [[ "$here" == "$there" ]] || warn "They differ. Run \`spares sync cloud\` and compare again."
  spares sync cloud

  if [[ -z "$domain" ]]; then
    bold "Closing the server's public ports"
    echo "You reached the server over Tailscale above. This allows only Tailscale in, so SSH to"
    echo "$ip stops working; log in with \`ssh $user@$host\` instead. If Tailscale ever breaks, use"
    echo "the VPS provider's web console, which can also reset root's password."
    if confirm "Close them?"; then
      ssh "root@$ip" 'command -v ufw >/dev/null || apt-get install -y -q ufw >/dev/null
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

  spares sync cloud              sync both ways and copy files (sync_interval_minutes = $interval
                                 syncs in the background, without copying files)
  spares sync conflicts          resolve notes tagged sync-conflict
  scripts/deploy_vps.sh update   update the server
EOF
}

update() {
  local remote_host
  remote_host="$(config_value remote_host)"
  [[ -n "$remote_host" ]] || die "remote_host is not set in $CONFIG_FILE. Run \`$0 init\` first."
  check_pushed
  bold "Updating $remote_host"
  scp -q "$REPO/scripts/setup_vps.sh" "$remote_host:/tmp/setup_vps.sh"
  ssh -t "$remote_host" 'sudo bash /tmp/setup_vps.sh; status=$?; rm -f /tmp/setup_vps.sh; exit $status'
  echo "Restart spares_serve here too, so both sides run the same version."
}

case "${1:-}" in
  init) shift; init "$@" ;;
  update) shift; update "$@" ;;
  *) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
