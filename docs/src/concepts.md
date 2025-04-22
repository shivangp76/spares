# Main Concepts

This application provides tools for a local spaced repetition system. It aims to improve upon the feature set provided by [Anki](https://apps.ankiweb.net/), while also providing a convenient CLI.

## Note vs Card

A note contains information about one topic. A card is a note with some information removed. Clozes are used to create a card from a note. Multiple clozes can be used in one card.

See <https://docs.rs/spares/latest/spares/spares/parsers/struct.NoteSettings.html> for a full list of note settings.

## Clozes

Clozes are added to parts of a note to create a card. They are used to omit certain parts of a note and the remaining parts will be used as a prompt for recall. Note that multiple clozes can be used in a single card if the information is spread out.

Clozes can also be nested, but only across different cards.

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
