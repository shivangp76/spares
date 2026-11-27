/**
 * What a lookup sends from the text around a tap: pure functions, tested in `text.test.ts`. The
 * host does the rest (`spares_core/src/parsers/impls/markdown_language_tools/dictionary/lookup.rs`).
 */

/** The most characters a lookup sends, as the host tries (`MAX_CHARS`). */
export const MAX_CHARS = 20;

/** Han and kana, written without spaces between words, so a lookup starts at the character tapped. */
export function isUnspaced(char: string): boolean {
  return /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}々〆ー]/u.test(char);
}

const WORD_CHAR = /[\p{L}\p{N}\p{M}'’-]/u;

/**
 * Where the lookup in `text` at `offset` starts: there for Han and kana, else at the start of the
 * word it is in, since languages written with spaces look up whole words.
 */
export function lookupStart(text: string, offset: number): number {
  const char = text.slice(offset, offset + 1);
  if (char === '' || isUnspaced(char)) return offset;
  let start = offset;
  while (start > 0) {
    const before = text.slice(start - 1, start);
    if (!WORD_CHAR.test(before) || isUnspaced(before)) break;
    start -= 1;
  }
  return start;
}

/** A note written by Add card from a dictionary entry: its word, with its reading, in a cloze. */
export function vocabularyNote(entry: { expression: string; reading: string; glosses: string[] }): string {
  const word =
    entry.reading && entry.reading !== entry.expression ? `${entry.expression}[${entry.reading}]` : entry.expression;
  const glosses = entry.glosses
    .slice(0, 3)
    .map(gloss => gloss.split('\n').join('; '))
    .join('; ');
  return glosses ? `{{${word}}}: ${glosses}` : `{{${word}}}`;
}
