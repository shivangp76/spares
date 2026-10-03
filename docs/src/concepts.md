# Main Concepts

This application provides tools for a local spaced repetition system. It aims to improve upon the feature set provided by [Anki](https://apps.ankiweb.net/), while also providing a convenient CLI.

## Note vs Card

A note contains information about one topic. A card is a note with some information removed. Clozes are used to create a card from a note. Multiple clozes can be used in one card.

See <https://docs.rs/spares/latest/spares/spares/parsers/struct.NoteSettings.html> for a full list of note settings.

## Clozes

Clozes are added to parts of a note to create a card. They are used to omit certain parts of a note and the remaining parts will be used as a prompt for recall. Note that multiple clozes can be used in a single card if the information is spread out.

Clozes can also be nested, but only across different cards.

Cloze settings can also reference other cards. For example, the `inh:` setting copies a source card's scheduling and review history onto a newly created card. See the [Inheriting SRS data from another card](workflows.md#inheriting-srs-data-from-another-card) workflow for details.

See <https://docs.rs/spares/latest/spares/spares/parsers/struct.ClozeSettings.html> for a full list of cloze settings.

See <https://docs.rs/spares/latest/spares/spares/parsers/struct.ClozeGroupingSettings.html> for a full list of cloze grouping settings.

## Tags

Tags are ways to connect notes together. They are generally used on a large scale, like grouping notes for a certain topic or class.

## Keywords

Keywords are used specifically for searching. For example, you may have multiple notes on integration by parts problems. You may want to add "Integration by parts" as a keyword to make it easy to find related notes. Keywords can be thought of as tags on a smaller scale. However, they are more flexible since they can contain longer phrases. It also removes the hassle of assigning these keywords to a hierarchy when the concept is too local. For example, it would be annoying to assign every little keyword like "Integration by parts" and "U substitution" to the "calculus" tag since that relation would rarely be used. Keywords can contain more variation within their name (like case insensitivity) which can be taken care of when searching. Hence, they are less rigid than tags. Keywords can also be used to store the source of the data. For example, you might want to add the author, chapter number, and theorem number as keywords. Keywords are separated by commas for searching convenience.

## Linked Notes

Each note can contain multiple linked notes, which are searched for using the parser specification. Notes are referenced based on their keywords. This is useful for connecting ideas together. Linked notes also support fuzzy finding, so notes can be referenced even if there are minor differences in phrasing of the keywords.

## Parsers

Parsers allow notes to be created in different markup languages. By default, spares ships with a Markdown and LaTeX parser. These are meant to be modified by the user. Note that a markup language can have multiple parsers. For example, you may have a parser called LatexMath for math notes and LatexChem for chemistry notes. This would allow you to have different preambles since chemistry LaTeX packages will not be needed for math notes and vice versa.

## Adapters

Adapters allow spares to interface with different spaced repetition software. By default, spares ships with an adapter for Anki and for spares itself.

## Scheduling

Cards are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS), which targets each card's desired retention. When a card in review is rated or rescheduled, FSRS proposes an interval and a small range of days around it. One day in that range is then chosen at random, weighted by these settings in `config.toml`:

```toml
# Prefer days that have fewer cards due.
load_balance = true
# Keep cards from the same note off the same and nearby days.
disperse_siblings = true

[easy_days]
# Relative workload per weekday. Here, Sundays get half the reviews of other days.
days_to_workload_percentage = { Mon = 1.0, Tue = 1.0, Wed = 1.0, Thu = 1.0, Fri = 1.0, Sat = 1.0, Sun = 0.5 }
# Days with no reviews, such as a vacation.
specific_dates = ["2026-12-25"]
```

Cards are only moved within their range, so these settings shift reviews by a few days at most. Cards in learning steps and cards due sooner than `minimum_interval` keep the interval FSRS gave them. If the settings rule out every day in the range, sibling dispersal is ignored first, and then the rest.
