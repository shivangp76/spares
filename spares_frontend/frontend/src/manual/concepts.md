# Main Concepts

This application provides tools for a local spaced repetition system. It aims to improve upon the feature set provided by [Anki](https://apps.ankiweb.net/), while also providing a convenient CLI.

## Note vs Card

A note contains information about one topic. A card is a note with some information removed. Clozes are used to create a card from a note. Multiple clozes can be used in one card.

See `spares_core::parsers::NoteSettings` for a full list of note settings.

### Note ids

Notes are named by short lowercase letter ids, such as `gir`, which also name their files (`typst/gir.typ`). Ids sort in creation order, and notes created together get ids next to each other. Ids are accepted in any case. How ids are chosen is documented on `spares_core::ids`.

## Clozes

Clozes are added to parts of a note to create a card. They are used to omit certain parts of a note and the remaining parts will be used as a prompt for recall. Note that multiple clozes can be used in a single card if the information is spread out.

Clozes can also be nested, but only across different cards.

Cloze settings can also reference other cards. For example, the `inh:` setting copies a source card's scheduling and review history onto a newly created card. See the [Inheriting SRS data from another card](workflows.md#inheriting-srs-data-from-another-card) workflow for details.

See `spares_core::parsers::ClozeSettings` for a full list of cloze settings.

See `spares_core::parsers::ClozeGroupingSettings` for a full list of cloze grouping settings.

## Tags

Tags are ways to connect notes together. They are generally used on a large scale, like grouping notes for a certain topic or class.

A card can also have tags of its own, which its note's other cards don't have, such as `hard` on the one card of a note you keep failing. Searching `tag=hard` finds a card when it or its note has the tag. A note's file shows its cards' tags on their clozes, right after the card's order, and pushing the file from the notes repository changes them:

```md
{{[o:1;tag:hard] Paris }} is the capital of {{[o:2,3;r:;tag:geography;rtag:hard] France }}.
```

`rtag:` is the reverse card's, for a cloze with `r:`. Writing `tag:` on a new cloze tags the card it creates. The tags of image occlusion and CLI cards are not written in the file, only kept by spares. Tag cards with `spares card edit --tags-to-add hard --ids 12` (or `--query`), with **Add tags to cards** on the web app's Cards page, or in the file, and take them away with `--tags-to-remove` or by deleting the key. A card's tags are listed beside its note's while reviewing, and in the Cards page's Card tags column.

A card you bury on two days without rating it in between gets the tag `buried` (see `TagBuriedConfig` on `spares_core::config`, the `[queue.tag_buried]` table of `config.toml`), so `tag=buried` lists the cards you keep putting off, which are usually ones to rewrite, split or drop. The days need not follow each other, burying it again on the same day doesn't count, and only burying it yourself counts, not sibling burying. Rating the card starts its count again but leaves the tag; undoing the bury takes the tag too. Set `days = 0` to turn it off.

## Keywords

Keywords are used specifically for searching. For example, you may have multiple notes on Spanish irregular verbs. You may want to add "Irregular verbs" as a keyword to make it easy to find related notes. Keywords can be thought of as tags on a smaller scale. However, they are more flexible since they can contain longer phrases. It also removes the hassle of assigning these keywords to a hierarchy when the concept is too local. For example, it would be annoying to assign every little keyword like "Irregular verbs" and "Subjunctive" to the "spanish:grammar" tag since that relation would rarely be used. Keywords can contain more variation within their name (like case insensitivity) which can be taken care of when searching. Hence, they are less rigid than tags. Keywords can also be used to store the source of the data. For example, you might want to add the author, book title, and chapter number as keywords. Keywords are separated by commas for searching convenience.

## Linked Notes

Each note can contain multiple linked notes, which are searched for using the parser specification. Notes are referenced based on their keywords. This is useful for connecting ideas together. Linked notes also support fuzzy finding, so notes can be referenced even if there are minor differences in phrasing of the keywords. A note's pane in the web app lists both the notes it links to and the notes that link to it.

A link also says which way the two notes depend on each other. Normally the note with the link builds on the note it links to, as in "by #lin[Fubini's theorem]". In Typst, `#lin(rel: "dependent")[the chain rule]` says the linked note builds on this one instead (as in "used in …"), and `#lin(rel: "related")[Riemann sums]` says neither does ("compare …", "see also …"). Which way a link points decides which cards wait for which when `new_after_linked_notes` or `reviews_after_linked_notes` is on, what `prerequisite_of` and `dependent_of` searches follow, and the arrows of the note graph, where related links are dotted and have none. `link_relation=dependent` finds the notes with such a link.

## Parsers

Parsers allow notes to be created in different markup languages. Spares ships with "markdown", "typst" and "latex-note" parsers, which are meant to be modified by the user (see `spares_core::parsers::impls`).

## Adapters

Adapters allow spares to interface with different spaced repetition software. Spares ships with an adapter for Anki and for spares itself.

## Scheduling

Cards are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS)-7, which targets each card's desired retention (FSRS-6 or older parameters in `fsrs_parameters` schedule with that version instead). Scheduling is set in `config.toml`, whose settings are documented on `spares_core::config::SparesExternalConfig`:

- New cards first go through learning steps, and forgotten cards through relearning steps, as in Anki (`learning_steps`, `relearning_steps`).
- FSRS-7 models memory within a day, so it can bring a card back later the same day outside its steps: an interval under 12 hours is given to the second, at least `fsrs_minimum_interval` (a minute by default), until the card has had `fsrs_maximum_same_day_reviews` reviews that day (5 by default). Set `fsrs_minimum_interval = 43200` to keep intervals to whole days.
- A review's due date is chosen among a few days around the one FSRS proposes, preferring days with fewer cards due, away from the note's other cards, and avoiding easy days (`load_balance`, `disperse_siblings`, `[easy_days]`; see `spares_core::schedulers::placement`).
- The `[queue]` table sets the order cards are shown in (`QueueConfig`), and `new_cards_daily_limit` and `reviews_daily_limit` how many.
- `[queue.presets.<name>]` tables give the cards of notes with some tags, or the cards a search finds (`query`), their own limits, orders, steps and sibling burying, as deck presets do in Anki (`QueuePreset`).
- `[goals.<name>]` tables name a set of cards to know by a date, like an exam's (`Goal`). `spares goal status` and the Statistics page show how each is going, part by part, and with a `retention` a card of the goal is never left due after the date with less recall than that on the day.

Changing these settings only affects cards as they are next rated. To apply them to every card now, run `spares card reschedule`, or click **Reschedule all cards** under Scheduler on the web app's Statistics page. Undoing it (`spares event undo <id>` with the id it prints, or Undo in the web app) restores the cards, not the settings. Cards given a due date by hand, postponed, advanced or moved by a break since their last review keep that due date, unless `reschedule_moved_cards = true`.

Before time away, `spares card break --from 2026-12-20 --days 10 --spread 7` moves the cards due during the break (and overdue ones, if it starts today) onto the 7 days after it, the ones that lose most by waiting first, and adds the break's days to `easy_days.specific_dates`. It shows what it would do and asks first; `--dry-run` only shows it.

Once you have a few hundred reviews, `spares card optimize` (or **Optimize parameters** on the Statistics page) fits FSRS's parameters to your own history, which usually predicts your memory better. `spares card optimize --apply` (or **Apply and reschedule**) saves them and reschedules every card. See `spares card reschedule --help` and `spares card optimize --help`.
