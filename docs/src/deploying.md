# Deploying a sync server

This sets up a small VPS that your phone (or any browser) reviews on, kept in two-way sync with your laptop. It was first done on Vultr's cheapest plan (512 MB, 10 GB, IPv6 only, $2.50/month) on 2026-10-14, from a Mac. The server is only reachable over [Tailscale](https://tailscale.com); nothing is open to the internet.

Two scripts do the work:
- `scripts/deploy_vps.sh` runs on the laptop. It builds spares for Linux here, uploads it, and sets up both sides.
- `scripts/setup_vps.sh` runs on the server (`deploy_vps.sh` uploads and runs it). It installs spares, a systemd service, swap and Tailscale's HTTPS.

The server never builds anything or downloads from GitHub, so it needs neither the memory to compile spares nor IPv4.

## What you need first

On the laptop:
1. **Tailscale**, logged in: `brew install tailscale && sudo brew services start tailscale && sudo tailscale up`, then open the link it prints.
2. **The cross-compiling tools:** `brew install zig && cargo install --locked cargo-zigbuild`. `deploy_vps.sh` adds the Rust target itself.
3. **bun**, which builds the web app (already needed for development).

On the phone: the Tailscale app, logged in to the same account.

If you have no Markdown or LaTeX notes (only Typst), pass `--no-latex` below. It skips LaTeX, inkscape and pandoc (about 3 GB), which a 10 GB server has no room for.

## 1. Make the server join your tailnet when it boots

Your laptop may not be able to reach the server's own address (an IPv6-only server, and no IPv6 at home), so the server joins your tailnet itself on its first boot.

1. Create an auth key at <https://login.tailscale.com/admin/settings/keys> → **Generate auth key**. Leave **Reusable** and **Ephemeral** off, then copy the key (`tskey-auth-...`).
2. Run:
   ```sh
   scripts/deploy_vps.sh cloud-init
   ```
   It prints a short script. Copy it and replace `TSKEY-PLACEHOLDER` with your key.

The name the server gets on your tailnet is `spares`. To use another, pass it (`cloud-init <name>`), and use that name everywhere below.

## 2. Create the server on Vultr

In the Vultr dashboard, click **Deploy** → **Deploy New Server**, then:

| Setting | Choose |
|---|---|
| Type | **Cloud Compute** (shared CPU) |
| Location | One that offers the $2.50 plan. Only some locations do. |
| Plan | **1 vCPU, 0.5 GB, 10 GB, 0.50 TB, $2.50/mo, IPv6 only** |
| Operating system | **Debian 13** (x64) |
| Auto Backups | Off. The laptop has the database and the deploy backs it up. |
| Cloud-Init User-Data | Enable it, and paste the script from step 1 (with your key) |
| SSH keys | None needed. You log in over Tailscale. |
| Server 1 Hostname | `spares` |
| Server 1 Label | `spares` (only shown in Vultr's dashboard) |

Click **Deploy Now**. After a minute or two, the server shows up as `spares` at <https://login.tailscale.com/admin/machines>. If your tailnet already had a device called `spares`, the new one is `spares-1`; use that name below.

**If you forgot the cloud-init script:** open the server in Vultr, copy the root password from its **Overview** page, click **View Console**, log in as `root` (type the password, since pasting often doesn't work there), and run:
```sh
curl -fsSL https://tailscale.com/install.sh | sh
tailscale up --ssh --hostname=spares
```
Type the `https://login.tailscale.com/a/...` link it prints into a browser on the laptop and approve the server.

## 3. Check that the laptop reaches it

```sh
tailscale status        # lists "spares"
ssh root@spares true    # may open a browser tab to approve; approve it
```
If ssh says root isn't allowed, open <https://login.tailscale.com/admin/acls>, add `"root"` to the `users` list in the `"ssh"` section, save, and try again.

## 4. Deploy

Start your local server (`spares_server` or `spares_serve`), then run, from the repo:
```sh
scripts/deploy_vps.sh init spares --no-latex
```
It takes 10–15 minutes. What it does, and what it asks:

1. **Backs up your database** to `~/.local/share/spares/spares-main.before-deploy-<time>.sqlite`.
2. **Builds** spares for the server and the web app (a 60 MB archive), and uploads them with `setup_vps.sh`.
3. **Sets up the server** as root: your user (it asks you to choose a **sudo password** for it, needed for `sudo` on the server later), swap, the `spares` service, and Tailscale's HTTPS.
   - **The first time on a tailnet**, Tailscale prints a link to enable HTTPS (`https://login.tailscale.com/f/serve?node=...`) and waits. Open it and enable **HTTPS**. **Don't enable Funnel**, which would put the server on the public internet.
4. **Sets up the laptop:**
   - stores the server's API key in the Keychain (service `spares-sync`);
   - asks **how often to sync in the background** (Enter keeps 15 minutes);
   - sets `remote_host`, `sync_url` and `sync_interval_minutes` in `~/.config/spares/config.toml` (keeping `config.toml.bak`);
   - offers to add `SPARES_REPLICA=0`, `SPARES_SYNC_ROLE=client` and `SPARES_SYNC_API_KEY` to `~/.config/spares/spares.zsh`. Say **yes**.
5. **Waits for you to restart the local server**, so it picks up the new settings. Stop it, open a new terminal (or `source ~/.config/spares/spares.zsh`), start it again, and press Enter. If your local server has an API key (`spares_serve`), the script reads it from the Keychain (`spares-api-key`) or asks for it.
6. **Uploads your database** after you confirm, then shows the note, card and review counts on both sides. They should match.
7. **Syncs once**, the normal two-way way.
8. **Offers to close the public ports** with `ufw`, so only Tailscale gets in. Say **yes**. If Tailscale ever breaks, Vultr's **View Console** still works.

Running `init` again is safe if something fails partway: it keeps the server's API key and doesn't duplicate settings.

## 5. Log in on the phone

With Tailscale on, open the address `init` printed, e.g. `https://spares.tail6814bc.ts.net`, and log in:
- **Server URL:** that same address.
- **API key:** the server's key. To show it on the laptop: `security find-generic-password -s spares-sync -w`.
- **Scheduler:** `fsrs`, unless you use another one on the laptop.

To review without a connection, add the page to the home screen (see [Reviewing offline](./getting_started.md#reviewing-offline)).

## Day to day

- **Run the local server as before.** `spares_server` (or `spares_serve`) syncs with the VPS when it starts and then every `sync_interval_minutes`. While it is stopped the laptop doesn't sync, and phone reviews wait on the server.
- **Sync now:** `spares sync cloud`. This also copies your templates, `config.toml` and the files your notes read to the server.
- **Conflicts** (both sides changed the same lines of a note) are tagged `sync-conflict`; go through them with `spares sync conflicts`.
- **Update the server** after changing spares: `scripts/deploy_vps.sh update`, then restart the local server, so both run the same version.
- **Log in to the server:** `ssh spares` (your user) or `ssh root@spares`. The service is `spares`: `sudo systemctl status spares`, `sudo journalctl -u spares -f`.

## On the server

| What | Where |
|---|---|
| spares | `/usr/local/bin/spares_server`, service `spares` |
| Web app | `/opt/spares/frontend` |
| Settings (API key, replica, role) | `/etc/spares/spares.env` |
| `setup_vps.sh`'s arguments, the URL | `/etc/spares/setup.conf`, `/etc/spares/url` |
| Database and data | the same path as on the laptop (`/Users/<you>` links to `/home/<you>`) |
| `config.toml` (copied from the laptop) | `~/.config/spares/config.toml` |

Spares uses about 15 MB of memory idle and 30 MB while sending the whole database, so 512 MB is plenty. `setup_vps.sh` adds 1 GB of swap for `apt`.

## Troubleshooting

- **"No spares server answers at 127.0.0.1:8080"**: the local server isn't running yet (it compiles first), or listens elsewhere (`SPARES_SOCKET_ADDRESS`).
- **The local server can't reach the VPS**: check that `tailscale status` on the laptop lists `spares`, and that Tailscale is running (`sudo brew services start tailscale`).
- **`spares sync cloud` is refused (401) by your local server**: it was started with an API key (`spares_serve`); the `spares` CLI sends `SPARES_API_KEY`, so set it (the `spares` function in `spares.zsh` does).
