# Plan: what's left of the offline web app

A browser can keep a copy of the collection and work offline as a sync replica of its own (Settings → This Device). It was tested in Safari in the iOS Simulator (2026-10-28, 2026-10-29 and 2026-10-31) and then on a real phone. How it works is documented where it is implemented:

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

## Found in the iOS Simulator, not fixed yet

- **pandoc's first card:** starting the pandoc worker takes 2–2.7 s in Safari (compiling `pandoc.wasm` is 0.2 s of it, the rest is starting pandoc), so the first Markdown card changed on a device takes that long to show its math (Chrome on a Mac: 165–180 ms). Later cards take 0.3–0.4 s, and an answer is ready in about 0.1 s once the front has shown for a moment. A review now prepares the next card during the one before (`src/nextCard.ts`), which starts the worker then, so this should be left only for a review's first card and after the worker stopped (`IDLE_MS` in `src/local/pandoc.ts`). Not measured since.
- **Large images:** a card of a 4000×3000 Typst image occlusion takes 2.9–3.4 s the first time and 0.7–0.8 s after, and those of one made on the device from a plain 254 KB JPEG 1.1–1.2 s, plus about 3 s on a freshly loaded page to start the Typst compiler (any Typst card). If that is too slow on a phone, keep the drawn PNGs in `device.sqlite`.
- **On a slow link:** at 300 ms and 50 KB/s, "Downloading the collection…" showed for 5.5 minutes with no progress, and the files' count stayed at 579/580 for 5 minutes while `pandoc.wasm` downloaded.
- **Seen once, not reproduced (2026-10-28):** a device sync sent 4 changes for two notes made on the laptop that the device had only received (their image occlusions named files it didn't have), and reported them as edited here when the laptop deleted them. A possible cause is fixed: a note both sides changed was always sent back after the merge, with its cards, even when the merge was the server's text (`reconcile_cards`). Not seen on 2026-10-31: the notes the device only received kept the usn of their first sync through every step.
