# Deploying a sync server

This sets up a small VPS as a spares host with your account, that your phone (or any browser) reviews on, kept in two-way sync with your laptop: your laptop's host syncs its copy of your collection with the VPS's as one of your devices. It was first done on Vultr's cheapest plan (512 MB, 10 GB, IPv6 only, $2.50/month) on 2026-10-14, from a Mac. The server is only reachable over [Tailscale](https://tailscale.com); nothing is open to the internet.

Two parts do the work:
- `spares deploy` runs on the laptop, in the repo. It builds spares for Linux from that checkout, uploads it with rsync, and sets up both sides. It shows each step as it goes and the time each took at the end.
- `scripts/setup_vps.sh` runs on the server (`spares deploy` uploads and runs it). It installs spares, creates the host with your account, and sets up a systemd service, swap, the sandbox renders run in (bubblewrap) and Tailscale's HTTPS.

The server never builds anything or downloads from GitHub, so it needs neither the memory to compile spares nor IPv4.

## What you need first

On the laptop:
0. **A spares host with your account**, running: `spares host init --user <you>`, then `spares_server`, and `spares login` (see Getting Started, which also moves a collection from before hosts into it).
1. **Tailscale**, logged in: `brew install tailscale && sudo brew services start tailscale && sudo tailscale up`, then open the link it prints.
2. **The cross-compiling tools:** `brew install zig && cargo install --locked cargo-zigbuild`. `spares deploy` adds the Rust target itself.
3. **bun**, which builds the web app (already needed for development).

On the phone: the Tailscale app, logged in to the same account.

If you have no Markdown or LaTeX notes (only Typst), pass `--no-latex` below. It skips LaTeX, inkscape and pandoc (about 3 GB), which a 10 GB server has no room for.

## 1. Make the server join your tailnet when it boots

Your laptop may not be able to reach the server's own address (an IPv6-only server, and no IPv6 at home), so the server joins your tailnet itself on its first boot.

1. Create an auth key at <https://login.tailscale.com/admin/settings/keys> → **Generate auth key**. Leave **Reusable** and **Ephemeral** off, then copy the key (`tskey-auth-...`).
2. Run:
   ```sh
   spares deploy cloud-init
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

Start your local host (`spares_server`), then run, from the repo:
```sh
spares deploy init spares --no-latex
```
It takes 10–15 minutes. What it does, and what it asks:

1. **Backs up your database** next to it, in your collection's directory (`<SPARES_ROOT>/users/<you>/`).
2. **Builds** spares for the server and the web app, and uploads them with `setup_vps.sh` (about 50 MB) to `~/.cache/spares-bundle` on the server. It keeps that copy, so later deploys send only what changed.
3. **Sets up the server** as root: your user (who can use `sudo` on the server without a password), the laptop's time zone, swap, the `spares` service, and Tailscale's HTTPS. It creates a host at the same root as your laptop's, with your account, and **asks for the account's password there**. A server set up before hosts is moved into the host instead, with its database.
   - **The first time on a tailnet**, Tailscale prints a link to enable HTTPS (`https://login.tailscale.com/f/serve?node=...`) and waits. Open it and enable **HTTPS**. **Don't enable Funnel**, which would put the server on the public internet.
4. **Sets up the laptop:** asks **how often to sync in the background** (Enter keeps 15 minutes), and sets `remote_host` and `interval_minutes` in the `[sync]` table of your collection's `config.toml` (keeping the old one as `config.toml.bak` if it changed).
5. **Logs your laptop's host in to the server** (`spares sync login`), with the password you just gave it: it gets a sync-only token there and keeps replica `0`.
6. **Uploads your database** after you confirm, then shows the note, card and review counts on both sides. They should match.
7. **Syncs once**, the normal two-way way.
8. **Offers to close the public ports** with `ufw`, so only Tailscale gets in. Say **yes**. If Tailscale ever breaks, Vultr's **View Console** still works.

Running `init` again is safe if something fails partway: it keeps the server's host and doesn't duplicate settings.

## 5. Log in on the phone

With Tailscale on, open the address `init` printed, e.g. `https://spares.tail6814bc.ts.net`, and log in:
- **Server URL:** that same address.
- **Name** and **Password:** your account on the server.
- **Scheduler:** leave it empty for the server's default, unless you use another one on the laptop.

The session lasts a year from its last use, so the phone asks again about as often as it is new.

To use spares on the phone without a connection, add the page to the home screen, open it from there, and in **Settings → This Device** choose **Keep a Copy on This Device** (see [Keeping a copy on this device](./getting_started.md#keeping-a-copy-on-this-device)).

## Day to day

- **Run your local host as before.** `spares_server` syncs with the VPS when it starts, when the laptop wakes, every `sync.interval_minutes`, and 30 seconds after you stop reviewing or editing (`sync.push_delay_seconds`), so you can close the laptop and carry on on your phone. While it is stopped the laptop doesn't sync, and phone reviews wait on the server.
- **Sync now:** `spares sync cloud`. This also copies your templates, `config.toml` and the files your notes read to the server.
- **Conflicts** (both sides changed the same lines of a note) are tagged `sync-conflict`; go through them with `spares sync conflicts`, or on the Conflicts page of the web app on the machine or phone whose sync found them.
- **Update the server** after changing spares: `spares deploy update`, then restart the local host, so both run the same version. You never log in to the server yourself: it reaches it over SSH (Tailscale's), and installs there with `sudo`. The first update after hosts also moves a server set up before them into a host: it asks here for the password of your account on the server, imports its collection, and logs your laptop's host in to it (`spares sync login`). A phone that keeps a copy loads the new web app the next time it opens it online, before it syncs. It compiles only what changed, builds the web app and connects to the server meanwhile, and uploads only the files that differ from the last deploy (mostly part of the binary), so it takes a minute or two. It runs unattended, and sets the server's time zone to the laptop's again, e.g. after you move.
- **Log in to the server:** `ssh spares` (your user) or `ssh root@spares`. The service is `spares`: `sudo systemctl status spares`, `sudo journalctl -u spares -f`.

## On the server

| What | Where |
|---|---|
| spares | `/usr/local/bin/spares_server`, service `spares` |
| Web app | `/opt/spares/frontend` |
| The host's root (`SPARES_ROOT`) | `/etc/spares/spares.env`; the same path as on the laptop (`/Users/<you>` links to `/home/<you>`) |
| Its settings and accounts | `host.toml` and `accounts.sqlite` in the root; manage with `SPARES_ROOT=<root> spares_server host --help` |
| Your collection | `users/<you>/` in the root |
| `setup_vps.sh`'s arguments, the URL | `/etc/spares/setup.conf`, `/etc/spares/url` |
| `config.toml` (copied from the laptop) | `users/<you>/config/config.toml` |
| The last deploy's build, which the next one updates | `~/.cache/spares-bundle` |

Spares uses about 15 MB of memory idle and 30 MB while sending the whole database, so 512 MB is plenty. `setup_vps.sh` adds 1 GB of swap for `apt`.

## Troubleshooting

- **The first `update` is slower**: it compiles only after connecting, as it doesn't know the server's architecture yet. `spares deploy` keeps that, the pandoc package and the time of each step in `~/.cache/spares-deploy`. If `~/.cache/spares-bundle` on the server is gone, the next deploy uploads everything again.
- **"No spares host answers at 127.0.0.1:8080"**: the local host isn't running yet (it compiles first), or listens elsewhere (`bind` in its `host.toml`).
- **The phone shows other streaks, or reviews on the wrong day of the heatmap, than the laptop**: the server is in another time zone (VPSs start in UTC), and spares counts days in the server's. Run `spares deploy update` to set it to the laptop's; `timedatectl` on the server shows it.
- **The local server can't reach the VPS**: check that `tailscale status` on the laptop lists `spares`, and that Tailscale is running (`sudo brew services start tailscale`).
- **`spares sync cloud` is refused (401) by your local host**: the CLI has no token for it; run `spares login`.
- **The sync is refused (401) by the server**: the laptop host's token there was revoked; run `spares sync logout`, then `spares sync login` again.
