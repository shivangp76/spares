# Workflows

## Live notes

Live notes let you write notes in a source file, such as lecture notes with prose between the notes, and keep them in sync with the database across repeated imports of that file. Add `live-sync-name` to the file's global settings, before any notes:

```md
<!--- spares: start --->
<!--- # live-sync-name: lecture_notes_501 --->
```

Then import the file with `spares import` after every edit, and finalize it with `spares import --strip-liveness` once the notes are stable. Edit a live note only in its source file: importing it from any other file, such as one from `spares note generate`, changing its text with `spares note edit`, and pushing a change to its text from the notes repository are all refused, since the next import of the source file would overwrite them. Its tags and keywords can still be changed in the web app, the notes repository or with `spares note edit`. How notes are matched, with a worked example, is in the module docs of `spares_cli::import`.

To fix a live note while reviewing it in the web app, edit its source file and run `spares import` on it. When you switch back to the review, the card is refreshed if its note changed, on the side you were on. If the browser stayed in view the whole time, press `R` (Reload Card, under More actions) instead. If the edit removed the card, the review moves on to the next one.

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
spares vcs init        # git
spares vcs init --jj   # jj, colocated with git (jj 0.30 or newer)
```

Your old notes directory is kept as `notes.pre-vcs-<time>`. Notes you had edited without syncing are put in their own commit (the `pre-vcs-local-edits` branch, or a jj change), which is printed. A file can also differ because spares changed the note without rewriting it (an undo, a tag rename), and pushing such a file would revert that change, so review them before pushing them. Other files from the old directory are copied into the clone as untracked files. Delete the old directory once you are done.

If the setup is interrupted, run it again: it continues where it stopped.

### With jj

`spares vcs init --jj` adds aliases to the clone's jj config (`jj spares-anki-pull` and `jj spares-anki-push` are described in [Syncing notes with Anki](workflows.md#syncing-notes-with-anki)):

```sh
jj spares-pull                  # get spares' changes, rebase your work onto them
jj new main@origin              # start a change, e.g. for an LLM to edit
# …edit…
jj diff                         # review
jj abandon                      # dislike it
jj describe -m "…" && jj spares-push   # like it: imports it into spares
```

`jj spares-push [REV]` fetches, then refuses if spares has newer notes (run `jj spares-pull` first) or if the revision or its unpushed ancestors have conflicts. Otherwise it moves your local `main` to the revision (default `@`, or `@-` if `@` is empty and not a merge) and pushes it. `jj spares-pull [REV]` rebases the branch of `@` (or of `REV`) onto spares' latest `main`. jj itself refuses to push a stale `main`, conflicted commits, and commits without a description.

New files are not tracked automatically in the clone (`snapshot.auto-track = "none()"`), so an editor's backup file never ends up in a push; use `jj file track` for a new note's file or a `README.md`.

Use jj commands only in a jj clone, rather than mixing in `git pull` and `git push`. `spares vcs init --repair-hooks` rewrites the aliases to their current version; aliases you edited are left alone.

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

- Notes at their canonical paths (`<parser>/<id>.<ext>`, such as `typst/gir.typ`), and a `README.md` at the root. Each file must still hold exactly that one note.
- **Creating a note:** add a file named `<parser>/new-<name>.<ext>` (such as `typst/new-bayes.typ`; `<name>` is lowercase letters, digits and `-`) holding one note without a `note-id`. Spares gives it an id and commits it under its canonical name on top of your push, so pull afterwards. The parser must already have notes in spares.
- **Deleting a note:** delete its file. The push message lists deleted notes, since their review history goes with them; `spares undo` restores them. Notes that linked to them are updated on their next render.
- A note's parser cannot be changed in place; to move a note to another parser, delete its file and create it anew, which loses its reviews.
- The linked-notes section is regenerated by spares, so edits to it are ignored.
- Only `main` and `anki` (see [Syncing notes with Anki](workflows.md#syncing-notes-with-anki)) can be pushed, and not both in one push.
- Never edit `notes-spares/` by hand (a PDF viewer's inverse search may open files there). Spares rewrites such files from the database, saving what it overwrites (see [Files spares overwrote](workflows.md#files-spares-overwrote)).

A push is imported in the order deletes, edits, creations, and completely or not at all: a single `spares undo` reverts all of it. After a push, spares may commit the notes as it writes them (for example with renumbered clozes) and render them, so pull before editing the same notes again. If what you pushed was already written the way spares writes it, there is nothing to pull.

### Rejected pushes

The reason a push was rejected is printed on lines starting with `spares:` (prefixed with `git:` or `remote:`), just above git's or jj's own error. jj follows them with `The remote rejected the following updates … (reason: push-to-checkout hook declined)` and a hint to check your permissions, which you can ignore; the `spares:` lines say what went wrong. Nothing is imported from a rejected push, and your commits stay in your clone.

| Message | What to do |
|---|---|
| spares has newer notes than your push is based on | Pull and rebase (`jj spares-pull` / `git pull --rebase`), resolve conflicts, push again |
| notes … changed in spares since the commit your push is based on | Spares committed their current version: pull and rebase, then push again |
| a path is not a note file, has a conflict marker or fails to parse | Fix the listed files and push again |
| adds note …, which does not exist in spares | Name the file `<parser>/new-<name>.<ext>` and remove its `note-id`; spares gives new notes their ids |
| a new note's file must not have a note id | Remove the `note-id` setting from the `new-*` file (copying a note's file keeps it) |
| note … is a live note, so only its tags and keywords can be changed here | Undo the edit, make it in the note's source file and run `spares import` there |
| the spares server is not running; start it to push | Start the spares server (a version with `spares vcs init`) and push again |
| this repository … does not belong to the spares server at … | Run `spares vcs init --repair-hooks` with the settings of the instance the repository belongs to |
| only `main` (imported into spares) and `anki` (written to Anki) can be pushed | Move `main` to your commit and push it (`jj spares-push`, or `git push origin HEAD:main`) |
| `main` and `anki` cannot be pushed together | Push them one at a time |
| this repository's hooks are older than pushing to `anki` | Run `spares vcs init --repair-hooks` |
| Anki must be open, with AnkiConnect, to push to `anki` | Open Anki and push again |
| Anki has newer notes than your push is based on | Spares committed them to `anki`: fetch (`jj spares-anki-pull`), merge `anki@origin` into your change, push again |
| … has the Anki id of … | The file is a copy of a note that is in Anki; remove `anki-note-id` from its custom data |
| writing to Anki failed partway | Some of the push is in Anki, and spares committed what Anki holds to `anki`: fetch, merge, push again |

### Files spares overwrote

Before spares overwrites or removes a file in `notes-spares/` that it did not write, it saves a copy to `notes-spares/.git/spares-overwritten/<time>/<path>`. The navbar then shows **Overwritten files saved**, and the server warns of them when it starts. Each copy is marked with whether the current file has the same contents. If it does (as when the same edit was pushed afterwards), nothing in it is lost. If it differs, the copy may hold an edit that is nowhere else: diff it with your clone's file, and copy what you want to keep into your clone and push it. Once you have checked them, delete the copies from the navbar's notice. Spares never deletes them itself.

While a push is being imported, other changes to notes (the web UI, the CLI) wait for it, for at most 30 seconds, and are then refused with a message to retry. Nothing is lost.

`spares vcs check` compares every note file with the database, and `spares vcs check --fix` rewrites what differs. Spares already does this for files changed outside it whenever it starts and before each push, so the check is only needed if you suspect a note file is out of date though git shows no change, as after a bug in spares. After reinstalling spares or changing its address, run `spares vcs init --repair-hooks`.

## Syncing notes with Anki

With notes in a [git or jj repository](workflows.md#notes-as-a-git-or-jj-repository), Anki is a branch: `anki` in spares' repository describes Anki's notes. Moving notes between Anki and spares is a merge between `anki` and `main`, which you review, pick from and resolve with git or jj like any other.

- **Which notes:** only Anki notes of the `Basic` note type with a `SparesId` field (as `spares migrate` sets it up: Front, Back, Keywords, SparesId, SparesParserName). Notes of other note types are ignored.
- **What Anki holds:** a note's first card, split into front and back at its first cloze; the rest of its markup is lost. So on `anki`, a note keeps its spares file as long as Anki holds what spares would write for it, and only notes changed in Anki show up as changed, as Anki's version (keeping the spares file's custom data and linked notes).
- **Notes never sent to Anki** stay on `anki` unchanged, so merging `anki` into `main` never deletes them. A note that was in Anki (it has `anki-note-id` in its custom data) and no longer is has been deleted there, and is deleted on `anki`.
- **Notes added in Anki** appear as `<parser>/new-anki-<Anki id>.<ext>` (the parser named by their `SparesParserName`, or the one most of your notes use), which merging into `main` creates. Spares then sets their `SparesId` in Anki if Anki is open; otherwise it still finds them by their Anki id.

git cannot run anything when you fetch, so `anki` is brought up to date first: `jj spares-anki-pull`, or `spares vcs refresh anki && git fetch`. Spares reads Anki through AnkiConnect while Anki is open, which sees Anki's latest changes, and otherwise from the collection at `ANKI_DB_PATH`. Pushing to `anki` needs Anki open, with AnkiConnect.

Only the spares instance on the machine with Anki has an `anki` branch; `spares sync cloud` does not carry it to a server.

### The first sync

The first refresh starts `anki` from `main`, with no earlier sync to compare against. So every note that differs between Anki and spares shows up as changed on `anki` to Anki's version, including notes that spares has the newer version of, and notes with `anki-note-id` that are no longer in Anki show up as deleted. Merging that into `main` as it is would revert or delete them. Review it first with `jj diff --from main@origin --to anki@origin` (or `git diff origin/main origin/anki`), and on `anki` restore spares' version of the notes Anki is behind on, then push that to Anki:

```sh
jj new anki@origin -m "Spares is newer"
jj restore --from main@origin markdown/gir.md typst/aab.typ   # the notes Anki is behind on
jj spares-anki-push
```

After that, merges are three-way and only bring what changed since the last one.

### From Anki to spares

```sh
jj spares-anki-pull                          # update and fetch `anki`
jj new main@origin anki@origin -m "From Anki"   # merge; review with `jj diff`, resolve conflicts
jj spares-push                               # import into spares
```

```sh
spares vcs refresh anki && git fetch
git switch main && git pull --rebase
git merge origin/anki                        # review, resolve conflicts
git push origin HEAD:main
```

The merge is three-way, so after the first one only notes that changed in Anki since the last merge come from `anki`, and only notes changed on both sides conflict. Check the push message for deleted notes: they lose their review history, and `spares undo` restores them.

### From spares to Anki

```sh
jj spares-anki-pull
jj new anki@origin main@origin -m "To Anki"     # merge `main` into `anki`
jj spares-anki-push                          # write to Anki
```

```sh
spares vcs refresh anki && git fetch
git switch -c to-anki origin/anki
git merge origin/main
git push origin HEAD:anki
```

A push to `anki` updates the Anki notes whose ids the files have (`anki-note-id`), adds to Anki every other note it adds or changes (including notes whose Anki note was deleted), and deletes the Anki notes whose files it deletes. So merging `main` sends the notes that changed on `main` since `anki` last had them. To send a note that has not changed, change its file on `anki` (a tag, say) and push.

After a push, spares records the Anki ids of the notes it added in their custom data (a commit on `main`) and commits them on top of `anki`, so fetch (`jj spares-anki-pull`) before merging again. A push is rejected if Anki changed since your `anki` was fetched; spares then commits Anki's changes to `anki` for you to merge. Anki has no transactions, so if writing fails partway, spares commits what Anki then holds to `anki`.

### Taking only some notes

Instead of merging, restore the notes you want from the other branch: `jj new main@origin && jj restore --from anki@origin markdown/gir.md`, or `git checkout origin/anki -- markdown/gir.md`. Notes you leave out are offered again by the next merge.

Repositories set up before `anki` existed need `spares vcs init --repair-hooks` once, for the hook that writes to Anki and the jj aliases.

Without a notes repository, `spares sync --from anki --to spares` still walks you through syncing notes interactively; see `spares sync --help`.

## Latex

Setting up `latexmk` and vimtex to compile notes as you edit them is described on `spares_core::parsers::impls::latex`.
