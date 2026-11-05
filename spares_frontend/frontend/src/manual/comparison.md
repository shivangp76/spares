# Comparison

## Anki Issues

These are the issues in Anki that spares was built to address, and how it does.

### File Management and Search
- Anki keeps notes in its database. Spares stores each note as a plain text file (such as `typst/gir.typ`), so notes can be searched with standard tools like `ripgrep`, edited in any editor, and kept in a git or jj repository (see [Notes as a git or jj repository](workflows.md#notes-as-a-git-or-jj-repository)).
- Anki renders LaTeX one note at a time as notes are viewed or checked. Spares renders notes in parallel in the background, each in a sandbox with limits.

### Format and Syntax Support
- Anki only supports HTML. Spares ships with Markdown, Typst and LaTeX parsers, and you can write your own (see [Parsers](concepts.md#parsers)).
- Anki's cloze delimiters are fixed to double curly brackets (`{{c1::text}}`), which clash with LaTeX math like `2^{2^{3}}` (see [this Reddit post](https://www.reddit.com/r/Anki/comments/a6grv9/anki_latex_cloze_1st_cloze_works_fine_2nd_cloze/)). Each spares parser chooses its own delimiters.
- Anki's clozes must be numbered by hand. Spares numbers them automatically (see `orders` on `spares_core::parsers::ClozeGroupingSettings` in the API docs).

### Architecture and Design
- Every spares card is a cloze card; front/back cards are a special case.
- Spares has no decks. Notes are grouped by tags, and `[queue.presets.<name>]` tables give the cards of tagged notes their own limits and steps, as deck presets do in Anki (see [Scheduling](concepts.md#scheduling)).
- Each parser's templates are files in `<config dir>/<parser>/templates/` (see `spares parser template --help`), so changing one updates every note, and LaTeX and Typst templates can be shared with your other documents.

### Image Occlusion
- Anki's image occlusion is its own note type. In spares it is a cloze type, so image clozes can sit in the same note as text clozes, and be grouped with them on one card.
- Hints can be shown over image occlusion clozes.
- A card's back can show its images without masks.

### Interface and Features
- Anki has no CLI. Spares has a full CLI, including reviewing, and a web app that is optional.
- Keywords let notes be found and linked to each other without a hierarchy (see [Keywords](concepts.md#keywords)):
  - Links are fuzzy, so they match despite small differences in phrasing, and partial references (e.g., "Ransford") work
  - No title field is required
  - One note can cite several sources
  - A new card can wait until the notes it links to are learned (`new_after_linked_notes` on `spares_core::config::QueueConfig`)
- Scheduling is pluggable, with FSRS built in, as Anki now also has.
- The web app can be installed on a phone, keeps a copy of the collection there and works offline. Sync goes through a server you host yourself (see [Deploying a Sync Server](deploying.md)), instead of AnkiWeb.

### Development Issues
- Anki's database structure is hard to work with (see <https://natemeyvis.com/on-ankis-database/> for technical details).

## Current Limitations of spares

- Spares is in alpha, so expect breaking changes.
- Cards cannot be flagged on their own. Flagging tags the whole note (`flagged_tag_name` on `spares_core::config::SparesExternalConfig`).
- There are no add-ons, and no site for finding shared decks. Notes can still be shared as files: export them to share from the Notes page of the web app, or with `spares note export --share` (`--zip` for one file), and import them in another collection with **Import** on its Notes page or `spares import`. Their images, audio and image occlusions go with them, and **Import** also adds the parsers the collection doesn't have (see [Sharing notes](workflows.md#sharing-notes)). Anki decks can be imported with `spares migrate`.
