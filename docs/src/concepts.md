# Main Concepts

This application provides tools for a local spaced repetition system. It aims to improve upon the feature set provided by [Anki](https://apps.ankiweb.net/), while also providing a convenient CLI.

## Note vs Card

A note contains information about one topic. A card is a note with some information removed. Clozes are used to create a card from a note. Multiple clozes can be used in one card.

See `spares_core::parsers::NoteSettings` for a full list of note settings.

### Note ids

Notes are named by short lowercase letter ids, such as `gir`, which also name their files (`typst/gir.typ`). Ids sort in creation order, and notes created together get ids next to each other. Ids are accepted in any case. Numeric ids from before `spares migrate ids` are listed in `id-map.tsv` in the data directory. How ids are chosen is documented on `spares_core::ids`.

## Clozes

Clozes are added to parts of a note to create a card. They are used to omit certain parts of a note and the remaining parts will be used as a prompt for recall. Note that multiple clozes can be used in a single card if the information is spread out.

Clozes can also be nested, but only across different cards.

Cloze settings can also reference other cards. For example, the `inh:` setting copies a source card's scheduling and review history onto a newly created card. See the [Inheriting SRS data from another card](workflows.md#inheriting-srs-data-from-another-card) workflow for details.

See `spares_core::parsers::ClozeSettings` for a full list of cloze settings.

See `spares_core::parsers::ClozeGroupingSettings` for a full list of cloze grouping settings.

## Tags

Tags are ways to connect notes together. They are generally used on a large scale, like grouping notes for a certain topic or class.

## Keywords

Keywords are used specifically for searching. For example, you may have multiple notes on integration by parts problems. You may want to add "Integration by parts" as a keyword to make it easy to find related notes. Keywords can be thought of as tags on a smaller scale. However, they are more flexible since they can contain longer phrases. It also removes the hassle of assigning these keywords to a hierarchy when the concept is too local. For example, it would be annoying to assign every little keyword like "Integration by parts" and "U substitution" to the "calculus" tag since that relation would rarely be used. Keywords can contain more variation within their name (like case insensitivity) which can be taken care of when searching. Hence, they are less rigid than tags. Keywords can also be used to store the source of the data. For example, you might want to add the author, chapter number, and theorem number as keywords. Keywords are separated by commas for searching convenience.

## Linked Notes

Each note can contain multiple linked notes, which are searched for using the parser specification. Notes are referenced based on their keywords. This is useful for connecting ideas together. Linked notes also support fuzzy finding, so notes can be referenced even if there are minor differences in phrasing of the keywords.

## Parsers

Parsers allow notes to be created in different markup languages. Spares ships with "markdown", "typst" and "latex-note" parsers, which are meant to be modified by the user (see `spares_core::parsers::impls`).

## Adapters

Adapters allow spares to interface with different spaced repetition software. Spares ships with an adapter for Anki and for spares itself.

## Scheduling

Cards are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS), which targets each card's desired retention. Scheduling is set in `config.toml`, whose settings are documented on `spares_core::config::SparesExternalConfig`:

- New cards first go through learning steps, and forgotten cards through relearning steps, as in Anki (`learning_steps`, `relearning_steps`).
- A review's due date is chosen among a few days around the one FSRS proposes, preferring days with fewer cards due, away from the note's other cards, and avoiding easy days (`load_balance`, `disperse_siblings`, `[easy_days]`; see `spares_core::schedulers::placement`).
- The `[queue]` table sets the order cards are shown in (`QueueConfig`), and `new_cards_daily_limit` and `reviews_daily_limit` how many.

Changing these settings only affects cards as they are next rated. To apply them to every card now, run `spares card reschedule`.

Once you have a few hundred reviews, `spares card optimize` fits FSRS's parameters to your own history, which usually predicts your memory better. `spares card optimize --apply` saves them and reschedules every card. See `spares card reschedule --help` and `spares card optimize --help`.
