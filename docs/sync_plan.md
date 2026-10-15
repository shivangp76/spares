# Sync plan

Open work on two-way sync and note ids. How sync works, and why, is documented on the code:
`spares_core::sync` and `spares_core::ids` (`cargo doc -p spares_core --document-private-items
--open`). This file only tracks what is not done or still needs thought.

## Known limitations

- **The rsync of rendered cards can overwrite the server's.** `spares sync cloud` copies `cards/`
  of parsers rendered on the server (not Typst) after the sync. A note edited on the phone between
  the sync and the copy has its newly rendered cards replaced by the laptop's older ones.
- **Without the notes repository, a note file edited during a sync can be overwritten** when the
  sync changes that note. `spares sync cloud` warns about edits not imported yet.
- **Conflicts can only be resolved on the laptop**, where the merge runs. The phone shows the
  `sync-conflict` tag, but its server has no conflict rows.
- **A full sync doesn't say how many changes each side would lose** before asking which direction.

## Open work

- **No test checks that every synced row gets its id from `allocate_note_ids` or `next_ids`.**
- **Untested:** the end-to-end steps from a phone browser. The rest was run with two local servers
  (a second `spares_server` on `:8090` with `SPARES_REPLICA=1`, `SPARES_SYNC_ROLE=server`, its own
  data and cache directories, and every `*_OUT_DIR` unset, or real rendered files get
  overwritten).

## A third replica

Not implemented: allocation refuses replicas other than `0` and `1`. A second laptop
(`SPARES_REPLICA=2`, `SPARES_SYNC_ROLE=client`) would take `y` as its tail letter in blocks opened
after it joins, with the laptop keeping `a`–`x`. Add it while all replicas are synced. Nothing
already allocated changes, because the packing doesn't depend on these rules.
