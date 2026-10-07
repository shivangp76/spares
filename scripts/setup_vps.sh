#!/usr/bin/env bash
# Sets up (or updates) a spares server on a Debian/Ubuntu VPS. Safe to re-run: it pulls, rebuilds
# and restarts, keeping the API key and data.
#
#   sudo bash setup_vps.sh --data-dir <local data dir> [--domain <domain>]
#
# --data-dir  The data directory on the machine you sync from (`spares sync cloud` copies it to the
#             same path on the server), e.g. /Users/you/.local/share/spares.
# --domain    Serve over HTTPS at this domain with Caddy. Point its DNS at this server first.
#             Without it, the server only listens on 127.0.0.1:8080 (use an SSH tunnel).
#
# Runs spares as the user who ran sudo, since `spares sync cloud` logs in as that user.
set -euo pipefail

REPO_URL="https://github.com/shivangp76/spares.git"
DATA_DIR=""
DOMAIN=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --domain) DOMAIN="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done
[[ $EUID -eq 0 ]] || { echo "Run with sudo." >&2; exit 1; }
[[ -n "${SUDO_USER:-}" && "$SUDO_USER" != root ]] || { echo "Run with sudo as the user that syncs, not as root." >&2; exit 1; }
[[ "$DATA_DIR" == /* ]] || { echo "--data-dir must be an absolute path." >&2; exit 1; }

SPARES_USER="$SUDO_USER"
USER_HOME="$(getent passwd "$SPARES_USER" | cut -d: -f6)"
SRC_DIR="$USER_HOME/spares"
CONFIG_DIR="$USER_HOME/.config/spares"
# Rendered files. Only this directory is served under `/files`, so the database stays private.
RENDERED_DIR="$USER_HOME/.cache/spares"
ENV_FILE="/etc/spares/spares.env"
as_user() { sudo -u "$SPARES_USER" -H bash -c "export PATH=\$HOME/.cargo/bin:\$HOME/.bun/bin:\$PATH; $1"; }

echo "==> Installing packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
# pandoc and LaTeX render markdown and latex-note cards. Typst cards are compiled by the browser.
# texlive-fonts-extra has the NewCM math font; inkscape converts SVGs for LaTeX.
apt-get install -y -q build-essential pkg-config git curl unzip rsync sqlite3 openssl nodejs npm \
  pandoc latexmk texlive-latex-recommended texlive-latex-extra texlive-fonts-recommended \
  texlive-xetex texlive-fonts-extra lmodern inkscape

# Building needs ~2 GB of memory, more than small VPSs have
if [[ $(free -m | awk '/^Mem:/{m=$2} /^Swap:/{s=$2} END{print m+s}') -lt 2500 && ! -f /swapfile ]]; then
  echo "==> Adding 2 GB of swap"
  fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile
  echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

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

echo "==> Configuring"
as_user "mkdir -p '$CONFIG_DIR' '$RENDERED_DIR'"
mkdir -p "$DATA_DIR" && chown "$SPARES_USER": "$DATA_DIR"
CONFIG_FILE="$CONFIG_DIR/config.toml"
if [[ -f "$CONFIG_FILE" ]] && grep -q '^render_browser_parsers' "$CONFIG_FILE"; then
  as_user "sed -i 's/^render_browser_parsers.*/render_browser_parsers = false/' '$CONFIG_FILE'"
else
  # Top-level keys must come before the first table
  as_user "{ echo 'render_browser_parsers = false'; cat '$CONFIG_FILE' 2>/dev/null || true; } > '$CONFIG_FILE.new' && mv '$CONFIG_FILE.new' '$CONFIG_FILE'"
fi

mkdir -p /etc/spares
API_KEY="$(grep -s '^SPARES_API_KEY=' "$ENV_FILE" | cut -d= -f2- || true)"
[[ -n "$API_KEY" ]] || API_KEY="$(openssl rand -hex 32)"
install -m 600 /dev/null "$ENV_FILE"
cat > "$ENV_FILE" <<EOF
SPARES_API_KEY=$API_KEY
SPARES_SOCKET_ADDRESS=127.0.0.1:8080
SPARES_DATA_DIR=$DATA_DIR
SPARES_CONFIG_DIR=$CONFIG_DIR
SPARES_CACHE_DIR=$RENDERED_DIR
SPARES_FILES_DIR=$RENDERED_DIR
SPARES_FRONTEND_DIR=$SRC_DIR/spares_frontend/frontend/dist
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
Restart=on-failure
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full

[Install]
WantedBy=multi-user.target
EOF

# Renders every note on the server, e.g. after the first `spares sync cloud`
cat > /usr/local/bin/spares-render <<'EOF'
#!/usr/bin/env bash
# Usage: sudo spares-render [--force]
set -euo pipefail
source /etc/spares/spares.env
force=false; [[ "${1:-}" == --force ]] && force=true
curl -fsS -X POST "http://$SPARES_SOCKET_ADDRESS/api/notes/generate_files" \
  -H "Authorization: Bearer $SPARES_API_KEY" -H 'Content-Type: application/json' \
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
else
  URL="http://localhost:8080 (after: ssh -L 8080:127.0.0.1:8080 $SPARES_USER@<this server>)"
fi

sleep 1
systemctl is-active --quiet spares || { journalctl -u spares -n 30 --no-pager; exit 1; }
HOST="${DOMAIN:-$(hostname -I | awk '{print $1}')}"
cat <<EOF

Spares is running.  Web app: $URL
API key: $API_KEY   (also in $ENV_FILE)

Next, on the machine you sync from:
  1. Add to its config.toml:   remote_host = "$SPARES_USER@$HOST"
  2. Copy your templates:      rsync -av ~/.config/spares/ $SPARES_USER@$HOST:$CONFIG_DIR/ --exclude config.toml
     and pandoc defaults, if config.toml sets parser.markdown.defaults_file:
                               rsync -av ~/.local/share/pandoc/ $SPARES_USER@$HOST:.local/share/pandoc/
  3. Run:                      spares sync cloud
  4. Then here, render cards:  sudo spares-render

Logs: journalctl -u spares -f      Update: re-run this script.
EOF
