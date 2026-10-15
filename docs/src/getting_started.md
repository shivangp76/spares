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

Spares ships with 3 main parsers: "markdown", "typst", and "latex-note".

The "markdown" parser serves as a guide for a basic parser, while the "latex-note" parser is more advanced. The "typst" parser is a middle ground. If you are creating your own parser, it is probably best to copy one of these as your starting template. The full specifications of these parsers can be found in the code documentation.

Note that creating or modifying the parsers will require recompiling the package.

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

## Typed answers

Add the cloze key `t` to type a card's answer before flipping it, like Anki's `{{type:cloze:Text}}`:
```md
The capital of France is {{[t] Paris}}.
```

| Setting | Comparison |
|---|---|
| `t` | A character diff, exact. |
| `t:ci` | A diff that ignores case. |
| `t:nc` | A diff that ignores accents and other combining marks. |
| `t:ci,nc` | Both. |
| `t:m` | No diff: what you typed is shown next to the revealed answer, for you to judge. Use it when you rephrase, or for math or an image. |

- Like `be`, `t` applies to a grouping, so it can be set on any cloze of a card.
- `<!--- # type-answer: ci --->` (`#se[type-answer: ci]` in Typst) applies it to all of a note's cards. A cloze's own `t` overrides it.
- Each answer cloze of a card gets its own input box: `Blank 1`, `Blank 2`, … in reading order, with the cloze's hint as placeholder. Its answer is compared with that box alone, so an answer containing a comma, such as `Washington, D.C.`, is not ambiguous.
- The expected answer is the cloze's text without its hint and markup. Emphasis, code, links and spans keep their text, and in Typst so does math (`#cl[$2x$][t]` expects `2x`). Nested clozes are part of the outer answer. A cloze without text, such as one holding only an image, gets no box.
- A reverse card (`r`) expects the rest of the note, so `t:m` suits it better.
- On the back, "Compare manually" switches between the diff and what you typed. As in Anki, you always pick the rating yourself.
- In the web app, the boxes are pinned to the bottom of the screen with the Check Answer button, so they stay in view however far the card is scrolled. Enter moves to the next box, and Enter in the last box (or Ctrl/Cmd+Enter in any) shows the answer. The boxes follow the Vim keybindings setting: with it on, a box starts in insert mode, Esc leaves for normal mode, and Enter still moves on in either mode. In `spares card review`, Esc skips the remaining boxes.

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

## Images

Notes refer to images by absolute path, e.g. `![](/Users/me/media/cell.png)` in Markdown or `#image("/Users/me/media/cell.png")` in Typst.

In the web app, paste or drop an image into a note's editor to insert it, from your files or from a web page. The image is uploaded to the server (an image dragged from a web page is downloaded by the server, since some browsers only hand over its URL), stored in `images` in the data directory, and inserted where it was pasted or dropped, in the note's syntax:
- Markdown: `![](…)`
- Typst: `#std.image("…", width: 80%)`
- LaTeX: `\includegraphics[…]{…}`, or `\includesvg[…]{…}` for an SVG. Your template must load the `graphicx` (or `svg`) package.

Supported formats are png, jpg, jpeg, gif, webp, bmp and svg. Images are named by their contents, so pasting the same image again reuses its file. A paste that also has text, such as cells copied from a spreadsheet, pastes the text. Like image occlusions made in the web app, uploaded images are stored on the server, not copied to your laptop.

## Audio

A link to an audio file is an audio clip. The card shows the link's text, and clients play the clip:
```md
How is "bonjour" pronounced? {{ [listen](/Users/me/media/bonjour.mp3) }}
```
```typst
#link("/Users/me/media/q.mp3")[🔊] Translate: #cl[#link("/Users/me/media/a.mp3")[🔊] good morning]
```

- Audio files are recognised by their extension: mp3, m4a, aac, ogg, oga, opus, wav and flac. Other links, including linked notes and images, are not audio.
- Paths must be absolute, as for images. A relative path gives an import warning, since neither a PDF viewer nor the server can find the file.
- In Markdown, leave a space between `{{` and a link at the start of a cloze, since `{{[` starts the cloze's settings.
- The link's text is typeset like any other text, so the PDF engine must support it. pdflatex, pandoc's default, fails on an emoji such as 🔊, and a font without it leaves the link blank. Use words such as `[listen](…)`, or an engine and font with the symbol.
- The LaTeX parser has no audio clips.

Which clips play:

- **The front** plays the clips it shows: those in the surrounding text, and in clozes of other cards that are not concealed (see `f:`).
- **The back** plays the clips it reveals: those in the card's answer clozes, and in other clozes if the back is the whole note (see `b:`). Clips already on the front are not played again.
- A clip inside a hidden cloze never plays on the front.
- **Replay** plays the front's clips, and then the back's if the card is flipped. In the web app, press `r` or click Replay in the audio bar above the card. In `spares card review`, pick Replay Audio.
- With `t`, audio links are not part of the expected answer, so `{{[t] [🔊](/m/x.mp3)}}` gets no input box.

To record a clip in the web app, click **Record audio** above a note's editor, record, and click Insert. The recording is uploaded to the server, stored in `audio` in the data directory, and a link to it is inserted at the cursor, e.g. `[listen](…)` in Markdown or `#link("…")[listen]` in Typst. Browsers that can record AAC (such as Safari) upload an m4a file; others upload WAV, since the formats they record are not ones clips can use. Recording needs the web app to be served over HTTPS or from localhost, as browsers only allow the microphone there.

In the web app, clicking an audio link inside the card also plays it. If the browser blocks autoplay before the page has been used, a Play button appears instead. Clips are fetched from the server's render assets: rendering a note records its clips, and `spares sync cloud` sends them, like the other files notes read. Clips are downloaded with the cards for offline review.

`spares card review` plays clips with the `audio_player` setting in `config.toml`, followed by the clip's path. It defaults to `afplay` on macOS and `mpv --no-video` elsewhere.

In a rendered PDF, the link points at the clip's file, so clicking it depends on the PDF viewer. Some viewers, such as sioyek 2.0, do not open links to local files.
