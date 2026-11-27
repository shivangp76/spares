# Language Tools

The `markdown-language-tools` parser is the Markdown parser with two tools for learning languages:

- **Readings** above text, such as furigana and pinyin: `漢字[かんじ]` shows かんじ above 漢字.
- **Dictionary lookup**: look a word up from a card in the web app, and make a card of it.

It is opt-in. Add it to your collection to use it:

```sh
spares parser add --name markdown-language-tools
```

Everything else works as in the Markdown parser: clozes, links, audio, images, image occlusions, CLI blocks and text to speech. A collection that doesn't add it is unchanged.

## Readings

Write a reading in brackets after its text, as in Anki's furigana fields:

```md
私は 日本語[にほんご]を 勉強[べんきょう]しています。
我在学习 中文[zhōngwén]。
{{[o:1]食[た]べる}} means "to eat".
```

- The reading goes over the letters right before the `[`: 食[た]べる puts た over 食 only. A space or punctuation starts the text it goes over, so write `私は 日本語[にほんご]`, not `私は日本語[にほんご]`, which would put にほんご over 私は日本語. The space is not shown.
- Links (`[text](url)`), spans (`[text]{.mark}`), footnotes, code and math are left alone.
- Readings work in clozes, and in typed answers: `{{[t]食[た]べる}}` accepts both 食べる and たべる.
- Text to speech reads kana readings rather than their kanji, and Chinese characters rather than their pinyin.

To show readings only on the back, the usual vocabulary card, add `readings` to the note's custom data:

```md
<!--- # custom-data: {"readings": "back", "language": "ja"} --->
```

## The note's language

Set `language` in the note's custom data (`"ja"`, `"zh"`, `"ko"`). Without it, the language is guessed from the text: kana means Japanese, Hangul Korean. Text in Chinese characters only has no language, and looks words up in every dictionary.

The language chooses the dictionaries that lookups use, and the PDF's font. It is separate from `tts`, which chooses the voice: a note can have both, `{"tts": "ja_JP,0.9", "language": "ja"}`.

## PDFs

PDFs are made with lualatex, whose luatexja typesets Japanese, Chinese and Korean, with fonts TeX Live includes (Harano Aji, Fandol and Un). Install TeX Live's `luatexja` on the machine that renders (it is in the full TeX Live).

To use other fonts, or change anything else about the PDF, write a pandoc defaults file at `markdown-language-tools/pdf-defaults.yaml` in your config directory:

```yaml
metadata:
  CJKmainfont: Hiragino Mincho ProN
  # The bold, for headings and **strong** text
  CJKoptions: [BoldFont=Hiragino Kaku Gothic ProN]
```

`markdown-language-tools/web-defaults.yaml` does the same for the cards the web app shows. The Markdown parser's defaults files aren't used.

## Dictionaries

Dictionaries are [Yomitan](https://github.com/yomidevs/yomitan) dictionary zips, the format most dictionaries for language learners are published in: JMdict and Jitendex for Japanese, CC-CEDICT for Chinese, and Wiktionary-based dictionaries for many other languages. spares includes none, since each has its own license; download the ones you want from their sites, or from the list in Yomitan's documentation.

```sh
spares language-tools dictionary import jitendex-yomitan.zip
spares language-tools dictionary import cc-cedict.zip --language zh
spares language-tools dictionary list
spares language-tools dictionary lookup 食べました
```

- A dictionary's language comes from its `index.json`, or `--language`, or else the script of its words.
- Each user imports their own, on their host. Dictionaries aren't synced, and aren't part of backups of the collection: import them again on another host.
- Importing a new revision of a dictionary replaces the old one. `disable` and `enable` leave one out of lookups without removing it.
- Each entry shows its dictionary's attribution, as licenses like JMdict's ask.

## Looking words up

On a card of this parser in the web app:

- Press **K** (as in Vim) to look up the word under the pointer, or the selected text.
- On a touchscreen, turn on **Look up words** above the card, then tap a word. Tapping elsewhere still shows the answer.
- The command palette's **Look up word…** asks for a word, which also works on cards shown as PDFs.

Japanese and Chinese are looked up from the character tapped, with the longest word the dictionaries have, and Japanese inflections are undone: 食べました finds 食べる, and says it was polite and past. Languages written with spaces look up the whole word, and find inflected forms only if the dictionary lists them.

In the popup, `j` and `k` move between entries, Enter adds a card for the one picked, and Esc closes it. **Add card** opens a new note with this parser, the word and its reading in a cloze, its meanings, the note's language, the word as a keyword, and the tags `vocab` and the card's note's. An entry whose word is already a keyword of a note says so, and opens it.

Lookup needs the host: a device that keeps a copy of the collection asks it, and can't look words up offline.
