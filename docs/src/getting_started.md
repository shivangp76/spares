# Getting Started

Clone this repository:
```sh
git clone https://github.com/shivangp76/spares.git
cd spares
cargo build --release
export PATH="$PATH:~/spares/target/release"
```

## Creating a parser (optional)

Spares ships with 3 main parsers: "markdown", "typst", and "latex-note".

The "markdown" parser serves as a guide for a basic parser, while the "latex-note" parser is more advanced. The "typst" parser is a middle ground. If you are creating your own parser, it is probably best to copy one of these as your starting template. The full specifications of these parsers can be found in the code documentation.

Note that creating or modifying the parsers will require recompiling the package.

## Starting the server

Run the following command in the terminal:
```sh
spares_server
```

### Running on a remote server (optional)

On a Debian/Ubuntu VPS, `scripts/setup_vps.sh` does all of the following, and also sets up the service, the web app and HTTPS. Run `sudo bash setup_vps.sh --data-dir <your local data dir> --local-home <your local home dir> --domain <domain>` on the server (or `--tailscale` instead of `--domain`, to reach it only over your tailnet), then follow the steps it prints.

#### Two-way sync

Notes can be edited and reviewed both on your machine and on the server (e.g. from your phone), and synced like Anki and AnkiWeb. Each side needs its own replica number and role, which the server requires at startup:

- your machine: `SPARES_REPLICA=0`, `SPARES_SYNC_ROLE=client`, and `SPARES_SYNC_API_KEY` set to the server's `SPARES_API_KEY`;
- the server: `SPARES_REPLICA=1`, `SPARES_SYNC_ROLE=server` (`setup_vps.sh` sets these).

Set `sync_url` in your `config.toml` to the server's address. Then `spares sync cloud` exchanges changes both ways. The first sync, or any sync after the two sides can no longer be merged, has to replace one side's database with the other's; the command says why and asks which (or pass `--full-upload` / `--full-download`). With `sync_interval_minutes`, your local server also syncs in the background, and the web app it serves has a Sync button.

A sync merges notes both sides changed: changes to different lines, tags and fields are combined, and reviews from both sides are kept. A note whose same lines were changed on both sides keeps your machine's text and is tagged `sync-conflict`; resolve it with `spares sync conflicts`. A note deleted on one side and edited on the other is kept. Changes made before a sync can no longer be undone. Live notes are read-only in the web app, since their text comes from files on your machine. See `docs/sync-plan.md` for the details.

#### Rendering locally

To keep a remote server's compute low, render notes on your own machine and push them with `spares sync cloud`. Besides syncing the database (or, without `sync_url`, replacing the server's with yours), it copies the rendered files to `remote_rendered_dir` on the server (default: `~/.cache/spares`), your templates, your `config.toml` (so reviews on the server are scheduled with your settings; see below), and every file that rendering recorded your notes as reading, such as preambles, figures and pandoc defaults. Those files are copied to the same paths relative to the server's home directory, so the server can render notes edited there. A file is only known once a note reading it has been rendered locally. On the server:

- `render_browser_parsers` is `false` in its `config.toml`: `spares sync cloud` copies yours with that changed, and without `remote_host`, `sync_url`, `sync_interval_minutes` and `remote_rendered_dir`. It overwrites the server's, so change settings on your machine. Parsers that render in the browser (such as "typst") are then not compiled on the server, and their cards' raw files are not written. Web clients compile these cards themselves. Keep this setting on where you render before syncing, since rendering is what records the files the notes read (figures, preambles, ...).
- Set `RAYON_NUM_THREADS=1` when starting `spares_server`, so generating note files cannot use every core while reviews are being served.

#### Markdown notes on the web

Rendering a Markdown note also writes an HTML file next to its PDF, which the web app shows instead of the PDF. This needs pandoc 3.12.1 or later, on your machine and on the server (`setup_vps.sh` installs it). If the HTML would lose content that only the PDF can show, such as raw LaTeX like a TikZ picture or math pandoc cannot convert to MathML, the web app shows the PDF instead, and rendering logs why (with `RUST_LOG=warn`).

- Set `"render": "pdf"` in a note's custom data to always show its PDF, or `"render": "html"` to show its HTML even if content is lost.
- Raw LaTeX meant only for the PDF, written as a ```` ```{=latex} ```` block, is left out of the HTML without falling back to the PDF.
- Style the HTML with a pandoc defaults file at `markdown/web-defaults.yaml` in the config directory, e.g. with `css: [~/.config/spares/markdown/web.css]`. The PDF's `defaults_file` is not used for HTML. Math macros defined in its `header-includes` are unknown to the HTML, so notes using them show their PDF; define macros in the note itself to have them apply to both.
- Notes rendered before upgrading show their PDF until they are rendered again, e.g. with `spares note generate --render --force-render`.

#### Reviewing offline

The web app can be installed as an app (e.g. "Add to Home Screen" on a phone) and opened without a connection. This needs the server to be reached over HTTPS, or at `localhost`.

- While the Review page is open and online, it downloads the next 200 cards due today and tomorrow for the default review, and for each filter a session is started with, along with what they display. New cards are included up to what is left of today's limit.
- When the server cannot be reached, a review shows those cards instead, and the navbar shows "Offline". Ratings and buries are saved on the device and sent once the server can be reached again, and the navbar shows how many are waiting. Other actions, such as editing a note or suspending a card, need the server.
- A card rated Again offline is not shown again until the rating has been sent, since the device does not schedule cards itself.
- A rating sent after a later review of the same card, e.g. from another device that synced first, is placed at its time in the card's history, which is then replayed. Such a rating cannot be undone from the review page.
- Undo takes back a rating or bury that has not been sent yet. Logging out keeps unsent actions, which are sent the next time you log in to the same server.

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

Rendered files are written to a per-parser directory, chosen by the first of these that applies:

1. The parser's own environment variable, if it points to an existing directory: `MARKDOWN_OUT_DIR`, `LATEX_OUT_DIR`, or `TYPST_OUT_DIR`.
2. `SPARES_CACHE_DIR`, if set.
3. The default cache directory, `$XDG_CACHE_HOME/spares` (`~/.cache/spares` if `XDG_CACHE_HOME` is unset).

The more specific variable wins, so `SPARES_CACHE_DIR` does **not** override a parser's `*_OUT_DIR`. If you run a separate instance of spares (e.g. for testing) next to your main one, unset the `*_OUT_DIR` variables for it too, or its rendered files will overwrite your main instance's. `spares_server` logs the directory each parser renders to when it starts (run it with `RUST_LOG=info` to see this).

The other directories can be overridden with `SPARES_CONFIG_DIR` (config file and parser templates), `SPARES_DATA_DIR` (database, and note and card text files), and `DATABASE_URL` (the database alone).

## Editing notes

Notes can be edited in the web UI, with `spares note edit`, or by editing their files.

To edit the files, keep them in a git or jj repository: run `spares note init-vcs` (or `spares note init-vcs --jj`), edit, commit, and push to import your changes. See [Notes as a git or jj repository](./workflows.md#notes-as-a-git-or-jj-repository).

# Reviewing

```sh
spares card review
```

## Image Occlusions

Here is an example of an note using the "markdown" parser with an image occlusion:
```md
<!--- spares: start --->

<!--- spares: note start --->
# Brain

This is a note about the brain.

<!--- spares: image occlusion start --->
<!--- original_image_filepath = "/misc/image_occlusions/brain.jpg" --->
<!--- clozes_filepath = "/misc/image_occlusions/brain_clozes.jpg" --->
<!--- spares: image occlusion end --->

<!--- spares: note end --->

<!--- spares: end --->
```

To create an image occlusion:

1. Run `spares_frontend image-occlusion`. This will automatically open the image occlusion editor in your web browser.
2. Click "SVG-Edit" -> "Change Background Image" and select the image you want to create clozes for.
3. Add markup and clozes to the appropriate layer. You can add cloze settings strings to clozes as needed.
4. Click "SVG-Edit" -> "Save SVG" to save your work.
5. In your note document, use the image occlusion snippet to insert the saved SVG.

The editor allows you to create multiple instances simultaneously, making it easy to work on different image occlusions at once. You can examine the generated SVG files to see exactly how the clozes are parsed. A more intuitive interface with `svg-edit` will be available in future updates.

## Audio/ Video

Refer to the `typst` parser and use `html` export.
