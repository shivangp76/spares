#!/usr/bin/env bash
# Sets up (or updates) a spares host on a Debian/Ubuntu VPS. Safe to re-run: it pulls, rebuilds
# and restarts, keeping the accounts and data.
#
#   sudo bash setup_vps.sh --root <local host root> --account <name> [--account-id <id>]
#                          [--local-home <local home>] [--domain <domain> | --tailscale]
#                          [--user <name>] [--bundle <dir>] [--no-latex] [--timezone <zone>]
#   sudo bash setup_vps.sh     # update, with the arguments of the last run
#
# Arguments are saved, and later runs start from them. `spares deploy` on your laptop runs
# this for you with --bundle, and sets up the laptop's side.
#
# --root        The root of the spares host on the machine you sync from (its SPARES_ROOT, e.g.
#               /Users/you/.local/share/spares). The host here gets the same path, so the paths
#               notes name images by are the same on both. The first run creates the host there
#               with your account, asking for its password.
# --account     Your account's name on the host here, and --account-id its id: make it your
#               account's id on the machine you sync from, so the directories match.
# --local-home  Your home directory on the machine you sync from, e.g. /Users/you. It is made a
#               symlink to the home directory here, where `spares sync cloud` copies the files your
#               notes read, so absolute paths to them (e.g. to a preamble) work on the server too.
# --domain      Serve over HTTPS at this domain with Caddy. Point its DNS at this server first.
# --tailscale   Serve over HTTPS on your tailnet only, with `tailscale serve`. No port is opened
#               to the internet. The first run prints a link to add this server to the tailnet.
#               Without either, the server only listens on 127.0.0.1:8080 (use an SSH tunnel).
# --user        The user spares runs as, and that `spares sync cloud` logs in as. Defaults to the
#               user who ran sudo, so pass it when running as root. A missing user is created,
#               with sudo and root's SSH keys.
# --bundle      Install spares from this directory (built elsewhere by `spares deploy`, which keeps
#               it in ~/.cache/spares-bundle) instead of building it here from GitHub. Building needs about 2 GB of memory and 10 GB of disk,
#               and GitHub cannot be reached from IPv6-only servers. Once used, every update needs one.
# --no-latex    Skip LaTeX, inkscape and pandoc (about 3 GB). Markdown and LaTeX notes edited on the
#               server are then not rendered there; Typst notes don't need them.
# --timezone    The server's time zone, e.g. America/New_York. Make it yours: spares counts days
#               (what is due today, new card limits, the review heatmap and its streaks) in the
#               server's time zone, and VPSs default to UTC. `spares deploy` passes the laptop's.
#
# The host here is the root of your collection's sync chain (replica 1): your laptop's host syncs
# with it as one of your devices (`spares sync login`).
#
set -euo pipefail

REPO_URL="https://github.com/shivangp76/spares.git"
# The arguments of the last run, for updating without them
SETUP_CONF="/etc/spares/setup.conf"
ROOT=""
ACCOUNT=""
ACCOUNT_ID=""
LOCAL_HOME=""
DOMAIN=""
TAILSCALE=false
PREBUILT=false
NO_LATEX=false
TIMEZONE=""
BUNDLE=""
SPARES_USER="${SUDO_USER:-}"
[[ $EUID -eq 0 ]] || { echo "Run with sudo." >&2; exit 1; }
if [[ -f "$SETUP_CONF" ]]; then
  # shellcheck source=/dev/null
  source "$SETUP_CONF"
  echo "==> Starting from the arguments of the last run ($SETUP_CONF)"
fi
while [[ $# -gt 0 ]]; do
  case "$1" in
    --root) ROOT="${2%/}"; shift 2 ;;
    --account) ACCOUNT="$2"; shift 2 ;;
    --account-id) ACCOUNT_ID="$2"; shift 2 ;;
    --local-home) LOCAL_HOME="${2%/}"; shift 2 ;;
    --domain) DOMAIN="$2"; shift 2 ;;
    --tailscale) TAILSCALE=true; shift ;;
    --user) SPARES_USER="$2"; shift 2 ;;
    --bundle) BUNDLE="$2"; PREBUILT=true; shift 2 ;;
    --no-latex) NO_LATEX=true; shift ;;
    --timezone) TIMEZONE="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done
[[ -n "$SPARES_USER" && "$SPARES_USER" != root ]] || { echo "Pass --user <name>, or run with sudo as the user that syncs. Not root." >&2; exit 1; }
[[ "$ROOT" == /* ]] || { echo "--root must be an absolute path." >&2; exit 1; }
[[ -n "$ACCOUNT" ]] || { echo "Pass --account <name>: your account on this host." >&2; exit 1; }
[[ -n "$ACCOUNT_ID" ]] || ACCOUNT_ID="$ACCOUNT"
[[ -z "$LOCAL_HOME" || "$LOCAL_HOME" == /?* ]] || { echo "--local-home must be an absolute path." >&2; exit 1; }
[[ -z "$DOMAIN" || "$TAILSCALE" == false ]] || { echo "Use --domain or --tailscale, not both." >&2; exit 1; }
[[ -z "$TIMEZONE" || -f "/usr/share/zoneinfo/$TIMEZONE" ]] || { echo "Unknown time zone: $TIMEZONE" >&2; exit 1; }
[[ "$PREBUILT" == false || -f "$BUNDLE/spares_server" ]] || { echo "This server runs spares built elsewhere. Update it with \`spares deploy update\` on your laptop, or pass --bundle." >&2; exit 1; }

if ! id "$SPARES_USER" >/dev/null 2>&1; then
  echo "==> Creating the user $SPARES_USER"
  adduser --disabled-password --gecos "" "$SPARES_USER"
  usermod -aG sudo "$SPARES_USER"
  if [[ -f /root/.ssh/authorized_keys ]]; then
    install -d -m 700 -o "$SPARES_USER" -g "$SPARES_USER" "/home/$SPARES_USER/.ssh"
    install -m 600 -o "$SPARES_USER" -g "$SPARES_USER" /root/.ssh/authorized_keys "/home/$SPARES_USER/.ssh/"
  fi
fi
# sudo without a password on this server, so `spares deploy update` runs unattended (root may not
# be able to log in then). The file is named without a dot, as sudo skips those.
SUDOERS=/etc/sudoers.d/spares
if [[ "$(cat "$SUDOERS" 2>/dev/null)" != "$SPARES_USER ALL=(ALL) NOPASSWD: ALL" ]]; then
  echo "==> Letting $SPARES_USER use sudo without a password"
  echo "$SPARES_USER ALL=(ALL) NOPASSWD: ALL" > "$SUDOERS.new"
  chmod 440 "$SUDOERS.new"
  visudo -cqf "$SUDOERS.new" && mv "$SUDOERS.new" "$SUDOERS"
fi

mkdir -p /etc/spares
{
  printf 'ROOT=%q\n' "$ROOT"
  printf 'ACCOUNT=%q\n' "$ACCOUNT"
  printf 'ACCOUNT_ID=%q\n' "$ACCOUNT_ID"
  printf 'LOCAL_HOME=%q\n' "$LOCAL_HOME"
  printf 'DOMAIN=%q\n' "$DOMAIN"
  printf 'TAILSCALE=%q\n' "$TAILSCALE"
  printf 'SPARES_USER=%q\n' "$SPARES_USER"
  printf 'PREBUILT=%q\n' "$PREBUILT"
  printf 'NO_LATEX=%q\n' "$NO_LATEX"
  printf 'TIMEZONE=%q\n' "$TIMEZONE"
} > "$SETUP_CONF"

USER_HOME="$(getent passwd "$SPARES_USER" | cut -d: -f6)"
# Checked before installing anything, and created before the data directory, which may be inside it
if [[ -n "$LOCAL_HOME" && "$LOCAL_HOME" != "$USER_HOME" ]]; then
  if [[ -L "$LOCAL_HOME" ]]; then
    [[ "$(readlink "$LOCAL_HOME")" == "$USER_HOME" ]] || { echo "$LOCAL_HOME already links to $(readlink "$LOCAL_HOME")." >&2; exit 1; }
  elif [[ -e "$LOCAL_HOME" ]]; then
    echo "$LOCAL_HOME exists. Move its contents into $USER_HOME, remove it and re-run." >&2; exit 1
  else
    mkdir -p "$(dirname "$LOCAL_HOME")" && ln -s "$USER_HOME" "$LOCAL_HOME"
  fi
fi
SRC_DIR="$USER_HOME/spares"
ENV_FILE="/etc/spares/spares.env"
USER_DIR="$ROOT/users/$ACCOUNT_ID"
as_user() { sudo -u "$SPARES_USER" -H bash -c "export PATH=\$HOME/.cargo/bin:\$HOME/.bun/bin:\$PATH; $1"; }

# Before the server (re)starts, as it reads the time zone then
if [[ -n "$TIMEZONE" && "$(timedatectl show -p Timezone --value)" != "$TIMEZONE" ]]; then
  echo "==> Setting the time zone to $TIMEZONE"
  timedatectl set-timezone "$TIMEZONE"
fi

export DEBIAN_FRONTEND=noninteractive
# bubblewrap is the sandbox every render runs in; git keeps each account's notes repository
PACKAGES=(curl rsync sqlite3 openssl python3 ca-certificates bubblewrap git)
# clang and llvm build SQLite's C for the copy of the collection browsers keep (spares_wasm)
[[ "$PREBUILT" == false ]] && PACKAGES+=(build-essential pkg-config git unzip nodejs npm clang llvm)
# pandoc and LaTeX render markdown and latex-note cards. Typst cards are compiled by the browser.
# texlive-fonts-extra has the NewCM math font; inkscape converts SVGs for LaTeX.
[[ "$NO_LATEX" == false ]] && PACKAGES+=(latexmk texlive-latex-recommended texlive-latex-extra \
  texlive-fonts-recommended texlive-xetex texlive-fonts-extra lmodern inkscape)
# Only the missing ones, so updates skip apt. Installed ones get upgraded with the rest of the
# system, e.g. by unattended-upgrades.
MISSING=()
for package in "${PACKAGES[@]}"; do
  [[ "$(dpkg-query -W -f='${Status}' "$package" 2>/dev/null)" == *"ok installed" ]] || MISSING+=("$package")
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
  echo "==> Installing packages"
  apt-get update -q
  apt-get install -y -q "${MISSING[@]}"
fi

# pandoc from its release instead of apt, whose pandoc is too old for the HTML of markdown notes
# (`--math-method` needs 3.11, and 3.12.0 breaks `--embed-resources`). Keep it at the version used
# locally, so notes rendered here get the same output. `spares deploy` reads this line.
PANDOC_VERSION="3.12.1"
if [[ "$NO_LATEX" == false && "$(pandoc --version 2>/dev/null | head -n1)" != "pandoc $PANDOC_VERSION" ]]; then
  echo "==> Installing pandoc $PANDOC_VERSION"
  PANDOC_DEB="$(mktemp --suffix=.deb)"
  if [[ "$PREBUILT" == true && -f "$BUNDLE/pandoc.deb" ]]; then
    cp "$BUNDLE/pandoc.deb" "$PANDOC_DEB"
  else
    curl -fsSL -o "$PANDOC_DEB" \
      "https://github.com/jgm/pandoc/releases/download/$PANDOC_VERSION/pandoc-$PANDOC_VERSION-1-$(dpkg --print-architecture).deb"
  fi
  apt-get install -y -q "$PANDOC_DEB"
  rm -f "$PANDOC_DEB"
fi

# Building needs ~2 GB of memory, more than small VPSs have. Without building, 1 GB keeps apt and
# LaTeX going on a 512 MB server.
SWAP_GB=2
[[ "$PREBUILT" == true ]] && SWAP_GB=1
if [[ $(free -m | awk '/^Mem:/{m=$2} /^Swap:/{s=$2} END{print m+s}') -lt $((SWAP_GB * 1024 + 450)) && ! -f /swapfile ]]; then
  echo "==> Adding $SWAP_GB GB of swap"
  fallocate -l "${SWAP_GB}G" /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

if [[ "$PREBUILT" == true ]]; then
  echo "==> Installing the bundled spares"
  install -m 755 "$BUNDLE/spares_server" /usr/local/bin/
  FRONTEND_DIR=/opt/spares/frontend
  rm -rf "$FRONTEND_DIR" && mkdir -p "$(dirname "$FRONTEND_DIR")"
  cp -r "$BUNDLE/frontend" "$FRONTEND_DIR"
  # Where `spares deploy update` uploads the next bundle, as the user, sending only what differs
  # from this one. The first deploy uploads it as root, into root's home.
  USER_BUNDLE="$USER_HOME/.cache/spares-bundle"
  if [[ "$(realpath "$BUNDLE")" != "$(realpath -m "$USER_BUNDLE")" ]]; then
    as_user "mkdir -p '$USER_BUNDLE'"
    rsync -a --delete --chown="$SPARES_USER:$SPARES_USER" "$BUNDLE/" "$USER_BUNDLE/"
  fi
else
  FRONTEND_DIR="$SRC_DIR/spares_frontend/frontend/dist"
  echo "==> Installing Rust and Bun"
  as_user 'command -v cargo >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal'
  as_user 'rustup update stable'
  as_user 'command -v bun >/dev/null || curl -fsSL https://bun.sh/install | bash'

  echo "==> Fetching the source"
  # The svgedit submodule is listed with an SSH URL, which needs a GitHub key
  GIT_HTTPS="-c url.https://github.com/.insteadOf=git@github.com:"
  if [[ -d "$SRC_DIR/.git" ]]; then
    as_user "cd '$SRC_DIR' && git pull --ff-only && git $GIT_HTTPS submodule update --init --recursive"
  else
    as_user "git $GIT_HTTPS clone --recurse-submodules '$REPO_URL' '$SRC_DIR'"
  fi

  echo "==> Building"
  as_user "cd '$SRC_DIR' && cargo build --release -p spares_server"
  as_user "cd '$SRC_DIR/spares_frontend/frontend' && bun install && bun run build"
  install -m 755 "$SRC_DIR/target/release/spares_server" /usr/local/bin/
fi

echo "==> Configuring"
# Created as the user where it can be (in the home directory, also through --local-home), so the
# directories on the way to it belong to the user too
as_user "mkdir -p '$(dirname "$ROOT")'" 2>/dev/null || mkdir -p "$(dirname "$ROOT")"
chown "$SPARES_USER": "$(dirname "$ROOT")"
# Earlier versions created directories as root, which stops `spares sync cloud` from copying files
# into them
find "$USER_HOME" -xdev -user root -exec chown -h "$SPARES_USER": {} +
spares_host() { as_user "SPARES_ROOT='$ROOT' /usr/local/bin/spares_server host $1"; }
# Whether the host has the account: a host whose first account was never made (an earlier run
# that stopped halfway) gets it now.
has_account() {
  [[ -f "$ROOT/accounts.sqlite" ]] &&
    [[ "$(sqlite3 "$ROOT/accounts.sqlite" "SELECT COUNT(*) FROM user WHERE id = '$ACCOUNT_ID'" 2>/dev/null)" == 1 ]]
}
if ! has_account; then
  echo "==> Creating the host in $ROOT, with the account $ACCOUNT"
  systemctl stop spares 2>/dev/null || true
  if [[ -f "$ROOT/accounts.sqlite" ]]; then
    CREATE="add-user '$ACCOUNT' --user-id '$ACCOUNT_ID' --admin"
  else
    CREATE="init --user '$ACCOUNT' --user-id '$ACCOUNT_ID' --bind 127.0.0.1:8080"
  fi
  # The password, typed (spares deploy init)
  if [[ -t 0 ]]; then
    spares_host "$CREATE" </dev/tty
  else
    echo "The host here needs your account's password: run \`spares deploy init\`, which asks for it." >&2
    exit 1
  fi
  has_account || { echo "The account $ACCOUNT wasn't created." >&2; exit 1; }
fi
HOST_TOML="$ROOT/host.toml"
# The settings this script manages; the rest of host.toml is the admin's
as_user "sed -i -E -e 's|^#? ?bind = .*|bind = \"127.0.0.1:8080\"|' -e 's|^#? ?frontend_dir = .*|frontend_dir = \"$FRONTEND_DIR\"|' '$HOST_TOML'"
CONFIG_FILE="$USER_DIR/config/config.toml"
as_user "mkdir -p '$USER_DIR/config'"
if [[ -f "$CONFIG_FILE" ]] && grep -q '^render_browser_parsers' "$CONFIG_FILE"; then
  as_user "sed -i 's/^render_browser_parsers.*/render_browser_parsers = false/' '$CONFIG_FILE'"
else
  # Top-level keys must come before the first table
  as_user "{ echo 'render_browser_parsers = false'; cat '$CONFIG_FILE' 2>/dev/null || true; } > '$CONFIG_FILE.new' && mv '$CONFIG_FILE.new' '$CONFIG_FILE'"
fi
# The token `spares-render` renders with
if [[ ! -s /etc/spares/render.token ]]; then
  install -m 600 /dev/null /etc/spares/render.token
  spares_host "token '$ACCOUNT_ID' --name 'spares-render on this server'" > /etc/spares/render.token
fi

install -m 600 /dev/null "$ENV_FILE"
cat > "$ENV_FILE" <<EOF
SPARES_ROOT=$ROOT
RAYON_NUM_THREADS=1
RUST_LOG=info
EOF

cat > /etc/systemd/system/spares.service <<EOF
[Unit]
Description=Spares server
After=network.target

[Service]
User=$SPARES_USER
EnvironmentFile=$ENV_FILE
ExecStart=/usr/local/bin/spares_server
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full

[Install]
WantedBy=multi-user.target
EOF

# Renders every note on the server. `spares sync cloud` copies the rendered files, so this is only
# for recovery, e.g. if they were deleted.
cat > /usr/local/bin/spares-render <<'EOF'
#!/usr/bin/env bash
# Usage: sudo spares-render [--force]
set -euo pipefail
force=false; [[ "${1:-}" == --force ]] && force=true
curl -fsS -X POST "http://127.0.0.1:8080/api/notes/generate_files" \
  -H "Authorization: Bearer $(cat /etc/spares/render.token)" -H 'Content-Type: application/json' \
  -d "{\"selector\":\"All\",\"immutable_note_ids\":null,\"overridden_output_raw_dir\":null,\"include_linked_notes\":true,\"include_cards\":true,\"generate_rendered\":true,\"force_generate_rendered\":$force}"
echo "Rendered."
EOF
chmod 755 /usr/local/bin/spares-render

systemctl daemon-reload
systemctl enable spares >/dev/null
systemctl restart spares

if [[ -n "$DOMAIN" ]]; then
  echo "==> Setting up HTTPS for $DOMAIN"
  if ! command -v caddy >/dev/null; then
    apt-get update -q
    apt-get install -y -q debian-keyring debian-archive-keyring apt-transport-https gnupg
    curl -fsSL 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -fsSL 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
    apt-get update -q && apt-get install -y -q caddy
  fi
  CADDYFILE=/etc/caddy/Caddyfile
  if [[ -f "$CADDYFILE" ]] && ! grep -q '# Managed by spares' "$CADDYFILE"; then
    cp "$CADDYFILE" "$CADDYFILE.bak"
    echo "Moved the existing Caddyfile to $CADDYFILE.bak"
  fi
  cat > "$CADDYFILE" <<EOF
# Managed by spares setup_vps.sh
$DOMAIN {
	encode zstd gzip
	reverse_proxy 127.0.0.1:8080
}
EOF
  if command -v ufw >/dev/null && ufw status | grep -q 'Status: active'; then
    ufw allow 80/tcp >/dev/null && ufw allow 443/tcp >/dev/null
  fi
  systemctl enable caddy >/dev/null
  systemctl reload-or-restart caddy
  URL="https://$DOMAIN"
elif [[ "$TAILSCALE" == true ]]; then
  echo "==> Setting up Tailscale"
  command -v tailscale >/dev/null || curl -fsSL https://tailscale.com/install.sh | sh
  # Prints a link to log in on the first run. `--ssh` lets you reach this server over the tailnet,
  # so its public SSH port can be closed.
  tailscale status >/dev/null 2>&1 || tailscale up --ssh
  tailscale set --ssh
  # The first time on a tailnet, this prints a link to enable HTTPS and waits until you do.
  tailscale serve --bg 8080
  URL="https://$(tailscale status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
else
  URL="http://localhost:8080 (after: ssh -L 8080:127.0.0.1:8080 $SPARES_USER@<this server>)"
fi

# Browsers may only call the API from the web app's own address.
if [[ "$URL" == https://* ]]; then
  as_user "sed -i -E 's|^#? ?public_url = .*|public_url = \"$URL\"|' '$HOST_TOML'"
  systemctl restart spares
fi
sleep 1
systemctl is-active --quiet spares || { journalctl -u spares -n 30 --no-pager; exit 1; }
# For `spares deploy`, which reads where the server is from here
echo "$URL" > /etc/spares/url
HOST="${DOMAIN:-$(hostname -I | awk '{print $1}')}"
[[ "$TAILSCALE" == true ]] && HOST="${URL#https://}"
cat <<EOF

Spares is running.  Web app: $URL  (log in as $ACCOUNT)
Its host is in $ROOT; manage accounts with \`SPARES_ROOT=$ROOT spares_server host --help\`.

Next, on the machine you sync from, whose host's copy of your collection syncs with this one:
  1. Log its host in to this one:  spares sync login $URL
  2. Optionally, in its config.toml:
       [sync]
       remote_host = "$SPARES_USER@$HOST"   # for \`spares sync cloud\` to copy rendered files
       interval_minutes = 15                # sync in the background
  3. Run once:  spares sync cloud --full-upload
     After that, \`spares sync cloud\` syncs both ways and copies the rendered files, your
     templates and the files your notes read.

Logs: journalctl -u spares -f      Update: sudo bash setup_vps.sh (no arguments needed)
EOF
