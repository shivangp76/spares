# Obsidian plan

Features Obsidian has that spares lacks, so they can be addressed later. Spares already has
Obsidian's graph view (`/graph`: force and layered layouts, groups, depth limits, unmatched links,
display and force settings) and its local graph (beside a note, in the note pane), so those are
left out.

Each item says what is missing, when it matters (use case), a small example, and how it could be
built. They are ordered by how much they matter for spares, where a link is not only for
navigation: the queue holds a card back until the notes it links to are learned, so a missing or
wrong link changes what is reviewed.

## Backlinks

A list of the notes that link to a note, beside its Linked Notes.

- **Missing.** The note pane (`NoteDetail.tsx`) lists only the notes this note links to. The notes
  linking to it are shown only as nodes of the local graph, which is collapsed until opened.
- **Use case.** Before rewriting or deleting a note, see what builds on it. While reviewing a
  forgotten card, see which notes are held back by it.
- **Example.** Note `gir` (keyword "Krebs cycle") is linked from `gis` and `hab`. Its pane shows
  "Linked from: gis, hab", each opening that note.
- **How.** The graph endpoint already walks `GraphRelation::Dependent`
  (`spares_core::api::note::search`). Return the dependents at depth 1 with the note, or call the
  graph endpoint with `ancestor_depth: 0, descendant_depth: 1`, and list them as Linked Notes are.
  `spares link list` could gain the same direction.

## Unlinked mentions

Notes whose text mentions a note's keyword without linking to it.

- **Missing.** Nothing finds links that were never written.
- **Use case.** A note uses "Krebs cycle" in its prose but does not link the note with that
  keyword, so its cards are not held back until the Krebs cycle is learned, and the graph misses
  the edge.
- **Example.** Note `gir`'s pane lists "Unlinked mentions: hac, hba", each with the matching
  sentence and a button that adds the link to that note.
- **How.** For each keyword of the note, search other notes' data with the full-text index
  (`spares_core::search`, ignoring case and accents), leaving out notes that already link to it.
  A `spares keyword mentions <keyword>` command, and a pane section in the web app. Adding the link
  needs the parser's link syntax (`ParserSyntax`), as the note editor's completion does.

## Renaming a keyword updates its links

- **Missing.** `spares keyword` has `list`, `search`, `ranking`, `unmatched` and `duplicate`, and
  the Keywords page has no rename. Links name keywords, so editing a keyword away can leave its
  links unmatched or, through fuzzy matching, move them to another note, without notice.
  Obsidian rewrites links when a file is renamed.
- **Use case.** Renaming "Krebs cycle" to "Citric acid cycle" in one note.
- **Example.** `spares keyword rename "Krebs cycle" "Citric acid cycle"` changes the keyword and
  every link written as "Krebs cycle" (or matching it within `linked_note_max_score`), listing
  the notes changed. Saving a note that removes a keyword other notes link to warns, naming them.
- **How.** Find the linking notes as for backlinks, rewrite the link text with the parser's
  syntax, and save every note in one transaction, so a rename is undone as one event. Live notes
  can only change in their source file, so list them instead of editing them.

## Hover preview of a linked note

- **Missing.** Clicking a link in a rendered card opens the note (`HtmlViewer.tsx`), but there is
  no preview on hover, which Obsidian's Page preview gives.
- **Use case.** While reviewing, check what a linked note says without leaving the card.
- **Example.** Pointing at a link to `gir` for a moment shows its rendered note in a popover.
  On a phone, a long press does the same.
- **How.** A popover with `CardRenderer` for the note's render, opened from the same link
  handling as clicks, with a delay. Reuse a cached render, as the note pane does.

## Quick switcher and command palette

- **Missing.** Pages are reached with `g` and a key, notes through Notes' query. There is no
  Cmd+O to open a note by keyword or id, and no Cmd+P listing every action.
- **Use case.** Opening a note mid-review, or finding an action without learning its key.
- **Example.** Cmd+O, type "kreb", Enter opens `gir`. Cmd+P, type "bury", lists Bury Card, Bury
  Note and Bury Card Until After….
- **How.** A dialog fuzzy-matching keywords and ids (the keyword search behind the editor's
  completion), and one over the shortcuts in `help/shortcuts.ts`, which would need each action's
  handler as well as its key.

## Embedding a note

- **Missing.** Obsidian's `![[note]]` shows another note inline. Spares only links.
- **Use case.** A card that needs a definition from another note shows it on the back, instead of
  the definition being copied into both notes.
- **Example.** In Markdown, `![[Krebs cycle]]` renders note `gir` in place, uncovered, on every
  card of the note that embeds it.
- **How.** A parser syntax for embeds, resolved by keyword as links are, and rendered from the
  embedded note's render. Rendering must be redone when the embedded note changes, as for the
  generated linked-notes section.

## Links to a part of a note

- **Missing.** Obsidian links to a heading or a block (`[[note#heading]]`, `^block`). Spares
  links whole notes, and no link names a card.
- **Use case.** A card builds on one cloze of a long note, but is held back until every card of
  that note is learned.
- **Example.** A link to `gir/2` holds a card back only until `gir`'s second card is learned.
- **How.** Allow a card suffix in links, as `inh:` already names cards (`aae/1`), and make the
  queue's prerequisite check and the graph use the card when one is given.

## Browsing

Smaller features for finding one's way around notes:

- **Bookmarks.** Starring notes to reopen them. Saved searches cover searches but not single notes.
- **Random note.** Opening a random note of a query, for browsing outside reviews.
- **Outline.** The headings of a long note, to jump between them in the editor.
- **Tabs and split panes.** Opening two notes side by side, as when comparing or splitting one.

## Content

What a parser can show, mostly for the Markdown parser, since Typst and LaTeX can do most of it:

- **Callouts** (`> [!note]`), **Mermaid diagrams** and **footnotes**.
- **Daily notes and note templates**: a note created from a template by a key, with today's date.
  Snippets cover some of this in an editor, but not in the web app.

## Left out

- **Canvas**: much work for little use in spaced repetition.
- **A plugin API**: custom parsers and the notes repository (`spares note init-vcs`) cover most
  of what plugins would do.
- **Publish**: spares is not for publishing notes.
- **File history**: undo and redo, and the notes repository, cover it.
- **Properties (typed frontmatter)**: `NoteSettings`, tags and keywords cover it.
