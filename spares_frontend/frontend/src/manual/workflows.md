# Workflows

## Live notes

Live notes let you write notes in a source file, such as lecture notes with prose between the notes, and keep them in sync with the database across repeated imports of that file. Add `live-sync-name` to the file's global settings, before any notes:

```md
<!--- spares: start --->
<!--- # live-sync-name: lecture_notes_501 --->
```

Then import the file with `spares import` after every edit, and finalize it with `spares import --strip-liveness` once the notes are stable. Edit a live note only in its source file: importing it from any other file, such as one from `spares note generate`, changing its text with `spares note edit`, and pushing a change to its text from the notes repository are all refused, since the next import of the source file would overwrite them. Its tags and keywords can still be changed in the web app, the notes repository or with `spares note edit`. How notes are matched, with a worked example, is in the module docs of `spares_cli::import`.

## Inheriting SRS data from another card

New cards normally start with fresh scheduling. If you create a card that tests the same material as an existing card — for example when splitting one note into several, or when importing material you already know — you can copy the source card's scheduling and review history onto the new card with the `inh:` cloze setting:

```md
The Mariana Trench is the deepest point in the Pacific Ocean {{[inh:aae/1] at about 10,935 meters}}.
```

Here the new card inherits from note `aae`'s first card. The rules are on `inherit` in `spares_core::parsers::ClozeGroupingSettings`.

## CLI cards

A CLI card is reviewed by running a command, such as a test suite, rather than by flipping a rendered document. Its syntax and how its score becomes a rating are documented on `spares_core::parsers::cli`.

## Fast note creation

Using snippets, such as through [LuaSnip](https://github.com/L3MON4D3/LuaSnip), can speed up note creation.

## Converting notes between parsers

For example,
```sh
spares import --to-parser="markdown" aaa.tex aab.tex
```

See `spares import --help` for converting notes kept in a git or jj repository.

## Putting cards off until after an exam

While reviewing a tag for an exam, some cards, such as long proofs, matter in the long run but not for the exam. On the Review page, **Bury Card Until After…** (`d`) asks for the exam's date the first time, then sets the card due the day after it. The date is remembered in this browser for the query or tag being reviewed, so the next card it is pressed on is put off without asking, and the button sits beside Bury Card and Bury Note above the answer on phones. Since the cards come back on a set date rather than after a set number of days, they land after the exam however close it gets. Change Date picks another one, and a date that has passed is forgotten.

Only the card is put off, not its siblings, since a note's other cards may still matter for the exam.

## Unbury cards matching query

```sh
spares card edit -q 'QUERY and c.user_buried' --special-state none
```
where QUERY is replaced with your query

## Bulk note actions

spares ships with fzf support which can be used to perform bulk note actions, such as tagging. This selector can also be used to find all notes with a certain tag by typing `tag: .*tag1`. For more than 1 tag, see `spares_query` for advanced querying.

Examples:
```sh
spares note edit --tags-to-add tag1 tag2 --files aaa.tex aab.tex
spares note edit --tags-to-remove tag1 tag2 --files aaa.tex aab.tex
```

## Getting notes created `n` days before a note

You might be reviewing an old note and realize you forgot the note. Let's say this note was taken during a lecture in class. Then, you may need to understand the notes created a few days before that note. For example, this note may build on an idea introduced a few days earlier in class. A relative date in the query finds all notes created `n` days before a note, so that you can perform an action on them, such as marking them as forgotten. You may also wish to filter the notes by the tag.

```sh
# Notes created in the 5 days up to note `gir`
spares note search 'created_at<=note[gir].created_at and created_at>=note[gir].created_at-5d'
# The same, only for notes tagged `biology`
spares note search 'tag=biology created_at<=note[gir].created_at and created_at>=note[gir].created_at-5d'
```

As a shell function, `get_notes_before $ID $NUM_DAYS`, where `$NUM_DAYS` defaults to 5:
```sh
get_notes_before() {
  spares note search "created_at<=note[$1].created_at and created_at>=note[$1].created_at-${2:-5}d"
}
```

See "Relative dates" in the search syntax reference (`spares_core/src/search/mod.rs`) for other relative dates, such as `today-1w` and `now-12h`.

## Visualizations

Print tags as a tree:
```sh
spares tag list --limit=9999 --tree
```

Print notes as graph:
```sh
spares note list --limit=9999 --graph
```

## Notes as a git or jj repository

Spares can keep your note files in a git repository, so you can branch them, let an LLM edit them, review the result with any tool, and keep or discard it. The database acts as the remote:

- `notes-spares/` in the data directory is spares' copy. Only the spares server writes to it, and it commits every change, so its `main` describes the database.
- `notes/` (and the `~/notes` symlink) becomes your clone of it.
- **Fetch** to get changes made in spares: web edits, `spares note edit`, undo, tag renames, renders.
- **Push to `main`** to import your changes into spares.

### Setup

The server must be running.

```sh
spares note init-vcs        # git
spares note init-vcs --jj   # jj, colocated with git (jj 0.30 or newer)
```

Your old notes directory is kept as `notes.pre-vcs-<time>`. Notes you had edited without syncing are put in their own commit (the `pre-vcs-local-edits` branch, or a jj change), which is printed. A file can also differ because spares changed the note without rewriting it (an undo, a tag rename), and pushing such a file would revert that change, so review them before pushing them. Other files from the old directory are copied into the clone as untracked files. Delete the old directory once you are done.

If the setup is interrupted, run it again: it continues where it stopped.

### With jj

`init-vcs --jj` adds two aliases to the clone's jj config:

```sh
jj spares-pull                  # get spares' changes, rebase your work onto them
jj new main@origin              # start a change, e.g. for an LLM to edit
# …edit…
jj diff                         # review
jj abandon                      # dislike it
jj describe -m "…" && jj spares-push   # like it: imports it into spares
```

`jj spares-push [REV]` fetches, then refuses if spares has newer notes (run `jj spares-pull` first) or if the revision or its unpushed ancestors have conflicts. Otherwise it moves your local `main` to the revision (default `@`, or `@-` if `@` is empty) and pushes it. `jj spares-pull [REV]` rebases the branch of `@` (or of `REV`) onto spares' latest `main`. jj itself refuses to push a stale `main`, conflicted commits, and commits without a description.

New files are not tracked automatically in the clone (`snapshot.auto-track = "none()"`), since only existing notes can be pushed; use `jj file track` for a `README.md`.

Use jj commands only in a jj clone, rather than mixing in `git pull` and `git push`.

### With git

```sh
git pull --rebase                       # get spares' changes
git switch -c llm-edit origin/main      # start a branch, e.g. for an LLM to edit
# …edit…
git diff origin/main                    # review
git switch main && git branch -D llm-edit        # dislike it
git commit -am "…" && git push origin HEAD:main  # like it: imports it into spares
```

### What a push can change

- Only existing notes, at their canonical paths (`<parser>/<id>.<ext>`, such as `typst/gir.typ`), and a `README.md` at the root. Each file must still hold exactly that one note.
- Create notes with `spares note add` or `spares import`, and delete them with `spares note delete`. A note's parser cannot be changed by pushing.
- The linked-notes section is regenerated by spares, so edits to it are ignored.
- Only `main` can be pushed.
- Never edit `notes-spares/` by hand (a PDF viewer's inverse search may open files there). Spares rewrites such files from the database, saving what it overwrites to `notes-spares/.git/spares-overwritten/`.

A push is imported completely or not at all. After a push, spares may commit the notes as it writes them (for example with renumbered clozes) and render them, so pull before editing the same notes again. If what you pushed was already written the way spares writes it, there is nothing to pull.

### Rejected pushes

The reason a push was rejected is printed on lines starting with `spares:` (prefixed with `git:` or `remote:`), just above git's or jj's own error. jj follows them with `The remote rejected the following updates … (reason: push-to-checkout hook declined)` and a hint to check your permissions, which you can ignore; the `spares:` lines say what went wrong. Nothing is imported from a rejected push, and your commits stay in your clone.

| Message | What to do |
|---|---|
| spares has newer notes than your push is based on | Pull and rebase (`jj spares-pull` / `git pull --rebase`), resolve conflicts, push again |
| notes … changed in spares since the commit your push is based on | Spares committed their current version: pull and rebase, then push again |
| a path is not a note file, adds or deletes a note, has a conflict marker or fails to parse | Fix the listed files and push again |
| note … is a live note, so only its tags and keywords can be changed here | Undo the edit, make it in the note's source file and run `spares import` there |
| the spares server is not running; start it to push | Start the spares server (a version with `spares note init-vcs`) and push again |
| this repository … does not belong to the spares server at … | Run `spares note init-vcs --repair-hooks` with the settings of the instance the repository belongs to |
| only `main` can be pushed | Move `main` to your commit and push it (`jj spares-push`, or `git push origin HEAD:main`) |

While a push is being imported, other changes to notes (the web UI, the CLI) wait for it, for at most 30 seconds, and are then refused with a message to retry. Nothing is lost.

`spares note vcs-check` compares the repository with the database, and `spares note vcs-check --fix` rewrites what differs. After reinstalling spares or changing its address, run `spares note init-vcs --repair-hooks`.

## Syncing notes with Anki

```sh
spares sync --from anki --to spares --dry-run
```

This walks you through syncing notes between spares and Anki. See `spares sync --help` for reviewing changes one at a time, limiting the sync to some notes, and selecting them with fzf.

## Latex

Setting up `latexmk` and vimtex to compile notes as you edit them is described on `spares_core::parsers::impls::latex`.
