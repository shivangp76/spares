# Plan: what's left of the offline web app

A browser can keep a copy of the collection and work offline as a sync replica of its own (Settings → This Device). How it works is documented where it is implemented:

- `spares_core::sync` (More replicas): enrollment, multiple clients, full syncs, when a device syncs, and undo.
- `spares_core::sync::run`: a normal sync's steps, which hold no transaction during the push, and a push whose answer was lost.
- `spares_core::ids`: note ids that follow the laptop's newest note.
- `spares_core::sync::devices` and `spares_core::sync::media`: enrollment, the config a device gets, and its files.
- `spares_core::platform`: config, templates, images, audio and image occlusions in a browser, and the HTML for web clients a device has downloaded.
- `spares_wasm`: what runs in the browser, and its storage, images, audio and image occlusions.
- `spares_server::route::create_local_router`: the routes a device answers itself.
- `vendor/README.md`: the sqlx patch.
- `spares_frontend/frontend/src/local/`: the web app's side, with `pandoc.ts` for Markdown cards changed on a device. `e2e/local_copy.ts` tests it.
- The manual's Getting Started page (Keeping a copy on this device), for using it.

Delete this file once the items below are done.

## 1. Test it

One plan for everything a device's copy does, run in Safari in the iOS Simulator (iOS 27), then the parts the Simulator can't do on a real phone. The e2e tests run the copy in desktop engines with a server on `localhost`. This adds what only a phone shows: a page frozen or killed while hidden, a lost or slow connection, the photo library, its storage, memory and speed, and the fixes for what the last runs found (2026-10-28, 2026-10-29 and 2026-10-31). Write down what fails as an item under 2 or 3 below, with the step's number, and leave the rest of the plan as it is.

### Setup

1. **Automated, on the laptop:** these pass.
   - `cargo test -p spares_core sync` and `cargo test -p spares_core image_occlusion` (`device_card_matches_rendered_card`: the device's PNG for a Typst image occlusion is byte for byte the laptop's, and its name changes with the card's clozes and side).
   - `cd spares_frontend/frontend && bun run build && bun test ./src/local/pandocWasm.test.ts` (pandoc.wasm's HTML is the `pandoc` CLI's: it is skipped without the server's pandoc version, so check it ran) and `bun run e2e` (all three engines: the copy offline, Typst image occlusions from the server and made on the device, a Markdown note with a cloze and an image occlusion made offline before and after an edit, its math as MathML with the cloze hidden, and marked without `pandoc.wasm`).
2. **A scratch laptop and server,** never the real ones: their own `SPARES_CONFIG_DIR`, `SPARES_DATA_DIR`, `SPARES_CACHE_DIR`, `SPARES_FILES_DIR` and `DATABASE_URL`, with `MARKDOWN_OUT_DIR`, `LATEX_OUT_DIR` and `TYPST_OUT_DIR` unset and the server's `SPARES_SYNC_ROLE=server`. Copy the real collection into it, without the config's `[sync]` section. Serve the build from the scratch server (`SPARES_FRONTEND_DIR`), not a second vite. Stop scratch servers by their PID: the user's own `spares_server` on 8080 is the same binary.
   - Give the server's config `render_browser_parsers = false`, as `spares sync cloud` does (`config::server_config`). Otherwise it renders Typst notes, which a VPS doesn't (it has no `typst`), its pushes wait for those renders, and the phone downloads every Typst note's PDFs again after each sync.
   - Without `sync.remote_host`, do what `spares sync cloud` does after each laptop sync (`push_files` in `spares_cli::sync::cloud`): copy the laptop's data directory (no database, `notes/` or `cards/`), the rendered files of its Markdown and LaTeX notes, its rendered image occlusions and its templates to the server, with mtimes (`rsync -a`), and the rendered files not over the server's newer ones (`rsync -a --update`).
   - Point the scratch laptop's Typst templates at a copy of `template.typ` (step 9 edits it), never the real `~/learning/preambles/template.typ`.
3. **A proxy** in front of the scratch server, which the Simulator's Safari uses, that can be stopped (offline), throttled (a slow connection, e.g. 400 kbit/s with 300 ms latency), cut mid-request, and can hold a `POST /api/sync/push` for a few seconds before the server sees it (11). The scratch laptop syncs to the server directly. "Offline" below means the proxy is stopped, and "online" that it runs again. Safari's `online` event never fires this way: the Simulator's network doesn't change.
4. **Web Inspector:** the Simulator's `webinspectord_sim` socket, with one session per page that logs the console (the copy's worker included) and evaluates. It shows the network, Cache Storage and localStorage. The device's database is readable at the Simulator's `…/FileSystem/spares/.opaque/*` (a 4096-byte header, then SQLite).
   - A native `confirm` or `alert` blocks every evaluation until tapped, and a headless Simulator can't tap: replace `window.confirm` and `window.alert` first. Stopping a copy reloads the page, so keep their text in `sessionStorage` (17).
   - Audio needs a user gesture: evaluate the click with `Runtime.evaluate`'s `emulateUserGesture`.
   - Edit a note's text through CodeMirror's view (`.cm-content` → `cmTile.view`, then `dispatch`), not by replacing its `innerText`, which adds a line at the end: two edits next to each other conflict (10).
5. **Test notes on the laptop,** synced to the server before step 1. On one disk, create the notes with image occlusions through the scratch server's API instead: a device finds image occlusions only under the server's directory, and the laptop moves those it saves into its own.
   - a Typst note with an image occlusion from a JPEG, with a few clozes, one with a hint;
   - a Markdown note with clozes, inline and display math, and an image;
   - a Markdown note with a cloze per line and the default `back_reveal` (its back is the note);
   - a LaTeX note, a note with audio, and a Markdown note with an image occlusion;
   - a Typst note with an image occlusion of a full-size camera photo (4000×3000), and one of an SVG;
   - with a `web-defaults.yaml` on the server, e.g. `number-sections: true`.
6. Put a few photos in the Simulator's library (`xcrun simctl addmedia booted …`), one of them 4000×3000.

### Steps

1. **Keep a copy.** Open the server's address and log in. Settings → This Device → Keep a Copy on This Device, with a name. The progress goes through Registering this device, Downloading the collection and Downloading files: note how long each takes, and the collection's size. Right after, without a reload, the navbar has the Sync button and no unsynced changes. On the laptop, `spares sync devices` lists the device under its name, with its number (2–4).
2. **Open offline, from cold.** Go offline, terminate Safari (`xcrun simctl terminate booted com.apple.mobilesafari`), and open the address again, as typed (`/`, not only `/review`). It loads from the service worker, the navbar says Offline, and every page answers from the copy: Review, Cards, Search, Statistics (its scheduling section doesn't say "Downloaded cards can still be reviewed"), a note's page, Conflicts, Settings (the device list says it can't load, and loads once online). Rendered cards show: LaTeX PDFs, Markdown HTML, Typst cards compiled on the device, and images. In a review (the Cards page has no player), the audio note's card has an audio bar and its clip plays.
3. **Online, but the server unreachable.** With the proxy answering but not reaching the server (or refusing connections), tap Sync: the navbar says Offline, not only the Sync button's title.
4. **Review offline.** Review 20 or more cards, with each rating. A card rated Again comes back after its step, in the same session. Undo a rating: the card comes back with its old schedule. Advance and Postpone work. The unsynced count goes up, and Statistics count today's reviews. Note how long the next card takes after a rating.
5. **Edit offline.**
   - Create a Typst note and a Markdown note with inline and display math and a cloze, with tags. Each gets an id after the laptop's newest note, with the device's letter (e.g. `hbph`). The Typst note's cards show at once. The Markdown note's cards show pandoc's HTML: the math as math, not as written, and each card's own cloze hidden. Note how long the first card takes to show its math (on a Mac: 165–180 ms, about 780 ms with the CPU 4× slower; in the Simulator's Safari 2.0–2.6 s, see 3) and the next ones (20–120 ms).
   - Focus a note's editor: the page doesn't zoom. Edit an existing note's text, then its tags. A live note's text is read-only, its tags aren't.
   - Insert image (next to Record audio) picks a photo from the library and inserts it at the cursor. It shows offline. (The picker needs a tap: in the Simulator, set the button's file input to a photo with a `DataTransfer`, and likewise for image occlusions.) Record audio if the Simulator has a microphone, and play it.
   - Insert image occlusion from a note's editor, with a photo from the library, at the phone's width (it isn't hidden below 640 px). The Image Occlusion page itself says it isn't available on the device, rather than failing.
   - Undo the last edit, then redo it.
6. **Review a device's Markdown cards.** Review the cards of the Markdown note from 5: once the front has shown for a moment, the answer shows pandoc's HTML at once (converted while the front was shown), and rating and the next card take no longer than with Typst cards. Then review Typst and Markdown cards changed on the device in one session (both compilers loaded), switch to another app for a minute (`xcrun simctl launch booted com.apple.Preferences`), and come back: the page isn't reloaded. If it is, stop the pandoc worker sooner (`IDLE_MS` in `src/local/pandoc.ts`).
7. **Typst image occlusions, offline.**
   - **From the laptop:** the Typst note from Setup 5's cards, in Cards and in a review: the front shows the image with the asked cloze in the answer color, its hint, and the others as the note's `front_conceal` says, and the back reveals it. They match the laptop's rendered card: the same clozes, colors and hint.
   - **Made on the device:** create a Typst note, insert an image occlusion from the library, draw two clozes, and create it. Both cards show, each hiding its own cloze.
   - **Edited on the device:** Edit image occlusion 1, move or add a cloze, and save. The card shows the new clozes without a reload. Edit it again, give a cloze a hint (`h:` in its cloze settings, which the clozes SVG keeps), and save: the card follows.
   - **Large images:** the 4000×3000 note's cards, then a few in a review. Note how long the first one takes. If slow, the next step would be to keep the drawn PNGs in `device.sqlite`.
   - **Errors:** the SVG image occlusion from Setup 5 shows the device's error naming the SVG (not the device's PNG), and the app keeps working.
   - **Other parsers:** the Markdown note with an image occlusion, edited on the device, shows its cards with the images the device draws. LaTeX notes edited on the device wait for the server.
8. **Sync on reconnecting.** Go online without touching anything: the copy syncs (the `online` event on a phone; in the Simulator, after 5 minutes idle), and the unsynced count goes to 0. Undoing a change from before the sync says so in a sentence, not Rust's debug form. On the laptop, `spares sync cloud`, then check:
   - the notes from 5 and 7, with the same ids, text and tags, and their images and audio on the server (uploaded images stay there, see `spares_core::api::image`);
   - the reviews from 4 and 6, each once (statistics or the review log), with the device's scheduling;
   - the Markdown note rendered by the server, and on the phone, after the next sync's file download, the server's render, not the device's (the `web-defaults.yaml` numbering shows in both);
   - the Typst occlusion notes rendered by the laptop as before, with rendered PNGs that match what the device showed. The device keeps drawing its own, and Cache Storage `spares-copy-render-assets` and localStorage `spares_copy_assets` have no `@image-occlusions/` entries.
9. **Changes from elsewhere.** On the laptop, edit a Markdown note the phone has, without rendering it there (in the web app or with `spares note edit`), add one, delete one, edit `template.typ` so Typst cards look different (e.g. a heading's color), render a Typst note, and `spares sync cloud`. On the phone, terminate Safari and open it: it syncs on opening, and shows the changes. The edited note's cards show the edit (the server's render, also in its file list's mtime). Typst cards still compile and use the new template, online and then offline from cold: the device's `render_asset` table and Cache Storage `spares-copy-render-assets` have its new hash, and nothing comes from `spares-offline-render-assets`. Make a change on the phone and sync: the server's `render_asset` row for the template still has the laptop's hash. Then sync the phone between the laptop's database upload and its file rsync (start `spares sync cloud` and sync the phone at once): a render asset it got as a 404 is fetched again at the next refresh, so Typst cards don't keep failing.
10. **Merges and conflicts.** With both sides synced, go offline. Edit the same note on both (lines with another between them: neighbouring lines conflict, as in git), the same line of another note on both, and on the phone a note the laptop deletes. `spares sync cloud` on the laptop first, then go online. The first note has both edits; the deleted note is back with the phone's edit, tagged `sync-conflict` with a conflict to resolve (an edit beats a delete, see `spares_core::sync::client`); and the second is tagged `sync-conflict`, with a badge next to Sync. Resolve it on Conflicts, sync, and `spares sync cloud`: the laptop has the resolved text.
11. **A sync cut short.** After each, the copy syncs normally: the next sync doesn't ask for a full one, the counts match the laptop's, and no review is there twice.
    - Make a few changes, tap Sync, and switch to another app straight away. Come back after a minute, and again after 10.
    - The same, but terminate Safari during the sync.
    - Stop the proxy during a sync, and cut a push's answer at the proxy after the server has it.
    - Run `spares sync cloud` on the laptop, with a change to send, while the proxy holds the phone's push (else the phone's small push usually lands first): the phone's push is refused and it starts over, without an error.
    - Terminate Safari while it downloads files (after 9, with a lot of new cards): the next opening finishes the download.
    - Throttle the proxy, then do steps 1 and 8 again on a fresh copy (after 16): they finish, and say what they are doing.
12. **When it syncs by itself.** After a change, leave the page open and untouched: it syncs after about 5 minutes. With no changes, it pulls every 15 minutes. Switching to another app after a change syncs. Switching away with nothing changed and a recent sync doesn't (the server's log of `/api/sync/` requests).
13. **The back-forward cache.** With a copy open, type another site's URL in the tab, then the server's address again: the new page gets the copy (no "browser storage (OPFS) is unavailable … InvalidStateError"), and the Sync button's title has no error. Go back and forward again: each page works.
14. **Logging out and in.** Log out: the copy and its files stay, and nothing syncs (a change leaves the unsynced count as it is). Log in to the same server without reloading: it syncs and downloads files again. Logging in to another server doesn't use the copy.
15. **Full syncs.** With everything synced, `spares sync cloud --full-upload` on the laptop. On the phone, sync during the server's processing of it: the 503 is retried, not shown as a failure. Then Sync asks which to keep. Make a change on the phone first, then download the server's: the change is gone and the copy matches the laptop. Uploading the phone's own is the other choice: only with nothing unsynced on the laptop, after which `spares sync cloud` there asks for a full download.
16. **Stopping, online.** Stop Keeping a Copy with an unsynced change: the change reaches the server, the device is no longer in `spares sync devices`, and the app asks the server for everything again. Set it up again: it gets a number again.
17. **Stopping, offline.** Offline, Stop Keeping a Copy: it says the sync failed (once, not "Syncing failed: Sync failed: …"), then deletes the copy on request, and names the number the server still lists. Online, Remove that device in Settings → This Device: the list no longer has it.
18. **Limits.** With three devices kept (the Simulator's Safari and two desktop browsers, or a second Simulator), a fourth is refused with the server's message alone, not "the sync server answered 400 Bad Request: Sync failed:". In a desktop browser with a copy, a second tab says "Open in another tab", and takes the copy once the first is closed. A private window can't keep a copy, and says why in a sentence (not Rust's debug form, `Opfs { operation: "get OPFS root", … }`), before enrolling. Safari's private browsing can't be opened in the Simulator without taps: a WebKit window from Playwright without a profile (`browser.newContext()`) fails the same way.
19. **Pages without a copy.** In a browser that doesn't keep a copy: it never requests `/pandoc/` (the network panel), and the Typst occlusion cards show the laptop's rendered PNGs through `GET /api/render-assets`. A review of cards the server has rendered starts no pandoc worker on the device either.
20. **Storage.** The size of the Simulator's Safari website data for the server (or `du` of its data directory) before and after the copy: about 62 MB more with `pandoc.wasm` (11 MB downloaded, kept uncompressed in Cache Storage), plus the collection and its files.

### On a real phone

What the Simulator can't do, on an iPhone after `spares deploy update` (back up the laptop's and the server's databases first: step 15 replaces the server's):

- Install from the Home Screen over HTTPS, and do 1–2 from there (its storage is its own, and Safari deletes a website's after 7 days without a visit).
- Airplane mode for 2, 8 and 11, and locking the phone during a sync.
- The camera, from Insert image and Insert image occlusion, and recording audio.
- Leaving the app unopened for over 7 days, then opening it offline: the copy and its unsynced changes are there. Then online, it syncs normally.
- The Firefox app: 1–14 again (its own storage, and when it freezes a hidden page). It can't be inspected, so look into a failure in Safari first.
- Speed, storage (Settings → Apps → Safari → Advanced → Website Data), memory (6), and whether the phone gets warm during the first download.

## 2. Retire the old offline review

After the phone check, in its own change. The old path downloads 200 cards and sends the ratings later, without scheduling on the device:

- `src/offline/review.ts` (queue downloads, `nextOfflineCard`, the outbox), `src/offline/db.ts`, the statistics cache (`src/offline/statistics.ts`), and their use in `ReviewPage.tsx`.
- `spares_core::api::offline_review` and its routes (`/api/review/queue`, `/api/review/sync`), once no device needs them.
- Before removing the outbox, send ratings with `/api/review/submit` on a device with a copy.
- `precompileNextCard` reads the downloaded queue. With a copy, it should ask the copy for the next card instead, so that Typst cards are compiled ahead of time again, and the next card's Markdown sides converted with pandoc (`convertAhead` in `src/local/pandoc.ts`) when its note was changed on the device, so the pandoc worker starts during the card before.
- Then the thin client's offline file caches (`sendKeepingCopy`, `clearOfflineCopies`) can go too, since only the copy keeps files (`spares-copy-files`), once it keeps every render asset itself: a render asset the copy has no row for works offline only from `spares-offline-render-assets`. Its rows now follow the server's at each sync, so this should no longer happen: check that nothing comes from it (step 9).

## 3. Smaller

- **Other tabs:** they say "Open in another tab". They could send their requests to the tab that has the copy, over a `BroadcastChannel`.
- **Undo past a sync:** undo stops at the last sync, which is why a device doesn't sync after each action.
- **Files:** each refresh lists every file (`GET /api/sync/files` without `since=`), so it also sees deletions. Fine for the files a VPS has, since Typst cards aren't rendered there.

Found in the iOS Simulator (Safari, iOS 27.0), 2026-10-28, and not fixed yet:

- **Seen once, not reproduced:** a device sync sent 4 changes for two notes made on the laptop that the device had only received (their image occlusions named files it didn't have), and reported them as edited here when the laptop deleted them. A replay of the same steps didn't do it. A possible cause is fixed: a note both sides changed was always sent back after the merge, with its cards, even when the merge was the server's text (`reconcile_cards`). Not seen on 2026-10-31 either: the notes the device only received (made through the server, with image occlusions) kept the usn of their first sync through every step.

Found in the iOS Simulator (Safari, iOS 27.0), 2026-10-29, and not fixed yet (the other findings of that run are fixed, and held on 2026-10-31 in steps 2, 9, 15, 17 and 18):

- **pandoc's first card (5, 6):** starting the pandoc worker takes about 2 s in Safari (compiling `pandoc.wasm` is 0.2 s of it, the rest is starting pandoc), so the first Markdown card changed on a device takes 2.0–2.6 s to show its math (Chrome on a Mac: 165–180 ms). Later cards take 0.1–0.3 s in the app (pandoc itself 46–75 ms), and an answer is ready at once once the front has shown for a moment. Starting the worker earlier is item 2's `convertAhead`. On 2026-10-31: 2.5–2.7 s for the first card each time the worker had stopped (`IDLE_MS`), 0.3–0.4 s for the next, answers ready in about 0.1 s.
- **Large images (7):** a card of the 4000×3000 Typst image occlusion takes 2.9–3.2 s the first time and 0.66 s after, and those of a 4000×3000 one made on the device from a plain 254 KB JPEG 1.1–1.2 s. If that is too slow on a phone, keep the drawn PNGs in `device.sqlite` (7). On 2026-10-31: 3.4 s for each of its cards the first time and 0.7–0.8 s after, plus about 3 s on a freshly loaded page to start the Typst compiler (any Typst card).

Found in the iOS Simulator (Safari, iOS 27.0), 2026-10-31, and not fixed yet. Everything else in 1–20 passed, except recording audio (5), which needs a tap to allow the microphone, and Setup 1's `pandocWasm.test.ts`, which was skipped (Homebrew's `pandoc` is 3.13, the server's 3.12.1).

- **On a slow link (11):** at 300 ms and 50 KB/s, "Downloading the collection…" showed for 5.5 minutes with no progress, and the files' count stayed at 579/580 for 5 minutes while `pandoc.wasm` downloaded.
- **Seen, not a bug:** Safari, sent to the background during a sync, was terminated after 34 s by iOS (RunningBoard: a LinkPresentation assertion of Safari's timed out, `0x2182BAAD`), unlike other switches of a minute or more. The page reloaded on returning and the copy synced normally (11). A real phone may do the same.
