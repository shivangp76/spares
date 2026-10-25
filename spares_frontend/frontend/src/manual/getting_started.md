# Getting Started

Clone this repository:
```sh
git clone https://github.com/shivangp76/spares.git
cd spares
cargo build --release
export PATH="$PATH:~/spares/target/release"
```

## Reference documentation

This manual covers how to set spares up and use it. Settings, note and cloze syntax, and how each part works are documented on the code, as API docs. Open them with:
```sh
cargo doc --workspace --no-deps --document-private-items --open
```
When this manual names an item such as `spares_core::parsers::NoteSettings`, search for it there. Every command's options are in `spares <command> --help`.

## Creating a parser (optional)

Spares ships with 3 main parsers: "markdown", "typst", and "latex-note". To create your own, see `spares_core::parsers::impls`.

## Starting the server

Run the following command in the terminal:
```sh
spares_server
```

### Running on a remote server (optional)

On a Debian/Ubuntu VPS, `scripts/setup_vps.sh` does all of the following, and also sets up the service, the web app and HTTPS. Run `sudo bash setup_vps.sh --data-dir <your local data dir> --local-home <your local home dir> --domain <domain>` on the server (or `--tailscale` instead of `--domain`, to reach it only over your tailnet), then follow the steps it prints. Re-running it with no arguments updates the server.

On macOS, `spares deploy init <server>` does it all from your machine instead: it builds spares here and installs it on a fresh VPS (`setup_vps.sh --bundle`, so the server needs neither the memory to build it nor GitHub), sets up your machine for two-way sync (below), uploads your database and closes the server's public ports, leaving Tailscale. A 512 MB server is enough with `--no-latex`, if you have no markdown or LaTeX notes. Install Tailscale on your machine and phone, and zig and cargo-zigbuild (`brew install zig && cargo install --locked cargo-zigbuild`), first. If your machine can't reach the server's address (an IPv6-only server), `spares deploy cloud-init` prints a script for the provider's cloud-init field that puts the server on your tailnet, and `init` then takes its Tailscale name. `spares deploy update` updates the server. [Deploying a sync server](./deploying.md) goes through it step by step, including creating the server on Vultr.

#### Two-way sync

Notes can be edited and reviewed both on your machine and on the server (e.g. from your phone), and synced like Anki and AnkiWeb. Each side needs its own replica number and role, which the server requires at startup:

- your machine: `SPARES_REPLICA=0`, `SPARES_SYNC_ROLE=client`, and `SPARES_SYNC_API_KEY` set to the server's `SPARES_API_KEY`;
- the server: `SPARES_REPLICA=1`, `SPARES_SYNC_ROLE=server` (`setup_vps.sh` sets these).

Set `url` in the `[sync]` table of your `config.toml` to the server's address. Then `spares sync cloud` exchanges changes both ways. The first sync, or any sync after the two sides can no longer be merged, has to replace one side's database with the other's; the command says why and asks which (or pass `--full-upload` / `--full-download`). With `sync.interval_minutes`, your local server also syncs in the background, and the web app it serves has a Sync button.

A sync merges notes both sides changed: changes to different lines, tags and fields are combined, and reviews from both sides are kept. A note whose same lines were changed on both sides keeps your machine's text and is tagged `sync-conflict`; resolve it with `spares sync conflicts`. A note deleted on one side and edited on the other is kept. Changes made before a sync can no longer be undone. Live notes are read-only in the web app, since their text comes from files on your machine. How sync works, and why, is documented on `spares_core::sync` (`cargo doc -p spares_core --document-private-items --open`).

Some limitations:
- Conflicts can only be resolved on your machine. The web app shows the `sync-conflict` tag but cannot resolve them.
- Without the [notes repository](./workflows.md#notes-as-a-git-or-jj-repository), a note file you edit while a sync runs can be overwritten if the sync changes that note. With it, nothing is lost.
- `spares sync cloud` copies its rendered cards of markdown and LaTeX notes to the server after the sync. If such a note is edited on the web in between, the server's newer cards are replaced by yours until the next `spares sync cloud`.

#### Rendering locally

To keep a remote server's compute low, render notes on your own machine and push them with `spares sync cloud`. Besides syncing the database (or, without `sync.url`, replacing the server's with yours), it copies the rendered files to `sync.remote_rendered_dir` on the server (default: `~/.cache/spares`), your templates, your `config.toml` (so reviews on the server are scheduled with your settings; see below), and every file that rendering recorded your notes as reading, such as preambles, figures and pandoc defaults. Those files are copied to the same paths relative to the server's home directory, so the server can render notes edited there. A file is only known once a note reading it has been rendered locally. On the server:

- `render_browser_parsers` is `false` in its `config.toml`: `spares sync cloud` copies yours with that changed, and without its `[sync]` table. It overwrites the server's, so change settings on your machine. Parsers that render in the browser (such as "typst") are then not compiled on the server, and their cards' raw files are not written. Web clients compile these cards themselves. Keep this setting on where you render before syncing, since rendering is what records the files the notes read (figures, preambles, ...).
- Set `RAYON_NUM_THREADS=1` when starting `spares_server`, so generating note files cannot use every core while reviews are being served.

#### Markdown notes on the web

Rendering a Markdown note also writes an HTML file, which the web app shows instead of the PDF. It needs pandoc 3.12.1 or later; see `spares_core::parsers::impls::markdown` for when the PDF is shown instead and how to style the HTML.

#### Reviewing offline

The web app can be installed as an app (e.g. "Add to Home Screen" on a phone) and used to review without a connection, if the server is reached over HTTPS or at `localhost`. What works offline is described on `spares_core::api::offline_review`.

### Migration (optional)

Spares ships with a script to migrate data from Anki. See `spares migrate --help`.

## Adding notes

Spares ships with a CLI to interact with the server. Its documentation can be found by running `spares --help`.

Using the CLI, we can add our parsers to the database. This is needed for creating notes that use these parsers.
```sh
spares parser add --name markdown
spares parser add --name latex-note
```

The CLI also provides an import functionality to add notes in bulk. For example, using the "markdown" parser, we can create a file called `notes.md` with the following contents:
```md
<!--- spares: start --->

<!--- # tags: geology, oceans --->
<!--- spares: note start --->
# Oceans

- The 5 oceans in the world are:
    - {{ [Pacific](li) }}
    - {{ Atlantic }}
    - {{ Indian }}
    - {{ Southern }}
    - {{ Arctic }}
<!--- spares: note end --->

<!--- # tags: geology, oceans --->
<!--- # keywords: Pacific Ocean --->
<!--- spares: note start --->
# Pacific Ocean

- The deepest point in the Pacific Ocean is {{the Mariana Trench}}

<!--- spares: note end --->

<!--- spares: end --->
```

We can import this file with the following command:
```sh
spares import --parser markdown ./notes.md
```
(See `spares import --help` for more options)

## Linked Notes

Note the usage of `[Pacific](li)` in the first note. This is a linked note. The keyword "Pacific" will be searched for across all other notes and the note with the closest match will be linked in that note document. In this case, the second note has the keyword "Pacific Ocean". Since this is the only other note, this is the closest match. This linking will be done when notes are rendered (see below).

## Typed answers

Add the cloze key `t` to type a card's answer before flipping it, like Anki's `{{type:cloze:Text}}`:
```md
The capital of France is {{[t] Paris}}.
```
The comparison options (`t:ci`, `t:nc`, `t:m`) and the rest are on `spares_core::parsers::TypeAnswer`.

# Render notes

You can render notes with the following command. This will create the following files for each note:

- The note's text file.
- The note's rendered file. For the markdown parser, this is a pdf file.
- For each card:
    - The card's text file.
    - The card's rendered file. For the markdown parser, this is a pdf file.

```sh
spares note generate --render
```

The note's text file will also contain the linked notes. The exact syntax of these files can be modified in the parser.

### Where files are written

Rendered files go to `~/.cache/spares` by default. Environment variables such as `SPARES_CACHE_DIR` and the parsers' `*_OUT_DIR` change this; see "Directories" on `spares_core::config`. If you run a separate instance of spares next to your main one, unset the `*_OUT_DIR` variables for it, or its rendered files will overwrite your main instance's.

## Editing notes

Notes can be edited in the web UI, with `spares note edit`, or by editing their files.

To edit the files, keep them in a git or jj repository: run `spares vcs init` (or `spares vcs init --jj`), edit, commit, and push to import your changes. See [Notes as a git or jj repository](./workflows.md#notes-as-a-git-or-jj-repository).

# Reviewing

```sh
spares card review
```

## Image Occlusions

Image occlusions are clozes drawn on an image. In the web app, click "Insert image occlusion" in a note's editor. Without the web app, run `spares_frontend image-occlusion`, which opens the editor in your browser, and insert the saved SVG into your note. The syntax is on `spares_core::parsers::image_occlusion`.

## Images

Notes refer to images by absolute path, e.g. `![](/Users/me/media/cell.png)` in Markdown. In the web app, paste or drop an image into a note's editor to upload and insert it (see `spares_core::api::image`).

## Audio

A link to an audio file is an audio clip. The card shows the link's text, and clients play the clip:
```md
How is "bonjour" pronounced? {{ [listen](/Users/me/media/bonjour.mp3) }}
```
Which clips play on each side, and the other rules, are on `spares_core::parsers::audio`. To record a clip in the web app, click **Record audio** above a note's editor (see `spares_core::api::audio`).

## Text to speech

The cloze key `tts` reads a cloze aloud with the device's voices, as Anki's `{{tts}}` does. The note setting `tts` reads the rest of the note too:
```md
<!--- # tts: en_US --->
Good morning in French: {{[tts:fr_FR] bonjour}}
```
The value is a language, then optionally a speed and a voice, such as `fr_FR,0.8,Thomas`, or `off`. The web app reads text aloud with the browser's voices, which **Settings** lists and can turn off. `spares card review` uses the `tts_command` of `config.toml`. The rules are on `spares_core::parsers::tts`.
