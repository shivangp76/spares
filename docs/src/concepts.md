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

Cards are scheduled with [FSRS](https://github.com/open-spaced-repetition/fsrs4anki/wiki/ABC-of-FSRS), which targets each card's desired retention.

New cards first go through learning steps, and forgotten cards through relearning steps, as in Anki. Again returns to the first step, Hard repeats the current one, Good moves to the next step or graduates the card, and Easy graduates it immediately. Steps are in seconds, shorter than a day, and can be empty:

```toml
learning_steps = [60, 600]  # 1 and 10 minutes
relearning_steps = [600]    # 10 minutes
```

When a card in review is rated or rescheduled, FSRS proposes an interval and a small range of days around it. One day in that range is then chosen at random, weighted by these settings in `config.toml`:

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

Changing these settings, the learning steps, the FSRS parameters, or the scheduler only affects cards as they are next rated. To apply the change to every card now, run:

```sh
spares card reschedule
```

This replays each card's review history to recompute its memory state, then places it again with the settings above. Suspended and buried cards are included and keep their state. It cannot be undone, so it asks for confirmation (skip it with `--yes`).

It always reschedules every card; there is no `--query`. Cards are placed one at a time, each seeing the due dates already given to the others, so rescheduling only some of them would leave the load spread across days unbalanced.

### Optimizing FSRS parameters

FSRS starts with default parameters fitted to many people's reviews. Once you have a few hundred reviews, parameters fitted to your own history usually predict your memory better:

```sh
spares card optimize
```

This fits the parameters to your review log and prints them next to two measures of how well each set predicts the reviews you actually did, where lower is better: the log loss, and the RMSE between predicted and actual recall rates. Only reviews on a later day than the card's previous review count, and cards whose history was cut short by an import are skipped. It does not change anything.

To save the new parameters and reschedule every card with them, run:

```sh
spares card optimize --apply
```

They are saved only if they predict your reviews better than the current ones. Like `spares card reschedule`, this cannot be undone, so it asks for confirmation (skip it with `--yes`). Optimizing again every month or so keeps the parameters in step with your history.

The parameters are stored in `config.toml` and can also be set by hand. Leave the list empty to use the defaults:

```toml
fsrs_parameters = [0.212, 1.2931, 2.3065, 8.2956, 6.4133, 0.8334, 3.0194, 0.001, 1.8722, 0.1666, 0.796, 1.4835, 0.0614, 0.2629, 1.6483, 0.6014, 1.8729, 0.5425, 0.0912, 0.0658, 0.1542]
```

Parameters from Anki work too, including 19 values from FSRS-5 or 17 from FSRS-4.5. Run `spares card reschedule` after changing them by hand.
