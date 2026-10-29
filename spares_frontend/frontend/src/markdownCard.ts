// A Markdown card's source as HTML, for a device that keeps a copy of the collection, which renders
// nothing (see `PendingRender` in CardRenderer). The server renders it with pandoc, whose Markdown
// `marked` doesn't know all of, so this handles what card sources are made of: clozes, written as
// pandoc's `[…]{.mark}` spans, the template's YAML metadata, and raw blocks such as
// ```` ```{=latex} ````. Math and other pandoc syntax show as they are written, until pandoc's own
// HTML replaces it (see src/local/pandoc.ts).
import {
  Marked,
  type TokenizerAndRendererExtension,
  type Tokens,
} from "marked";

const MARK_ATTRIBUTES = "{.mark}";

/** The length of the `[…]{.mark}` span `src` starts with, brackets in it paired, or `null`. */
export function markSpanLength(src: string): number | null {
  if (!src.startsWith("[")) return null;
  let depth = 0;
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    if (c === "\\") i++;
    else if (c === "[") depth++;
    else if (c === "]" && --depth === 0) {
      return src.startsWith(MARK_ATTRIBUTES, i + 1)
        ? i + 1 + MARK_ATTRIBUTES.length
        : null;
    }
  }
  return null;
}

const markSpan: TokenizerAndRendererExtension = {
  name: "markSpan",
  level: "inline",
  start: (src) => src.indexOf("["),
  tokenizer(src) {
    const length = markSpanLength(src);
    if (length === null) return undefined;
    const raw = src.slice(0, length);
    const text = raw.slice(1, -1 - MARK_ATTRIBUTES.length);
    return {
      type: "markSpan",
      raw,
      text,
      tokens: this.lexer.inlineTokens(text),
    };
  },
  renderer(token) {
    return `<mark>${this.parser.parseInline(token.tokens ?? [])}</mark>`;
  },
};

const RAW_BLOCK = /^\{=([\w-]+)\}$/;

const cardMarked = new Marked({
  extensions: [markSpan],
  renderer: {
    // pandoc's raw blocks: HTML is kept, other formats (LaTeX) are only for the PDF
    code({ text, lang }: Tokens.Code) {
      const format = RAW_BLOCK.exec(lang ?? "")?.[1];
      if (format === undefined) return false;
      return format === "html" ? `${text}\n` : "";
    },
  },
});

/** pandoc's YAML metadata block at the start of `source`, if any, removed. */
function withoutMetadata(source: string): string {
  return source.replace(
    /^\s*---\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/,
    "",
  );
}

export function markdownCardHtml(source: string): string {
  return cardMarked.parse(withoutMetadata(source), { async: false });
}
