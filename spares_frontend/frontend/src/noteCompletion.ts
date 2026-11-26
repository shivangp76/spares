import {
  acceptCompletion,
  autocompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
  completionStatus,
  moveCompletionSelection,
  startCompletion,
} from "@codemirror/autocomplete";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  type Extension,
  keymap,
  Prec,
  StateEffect,
  ViewPlugin,
  type ViewUpdate,
} from "@uiw/react-codemirror";
import {
  getCardsForNote,
  getNote,
  listKeywords,
  searchNotesPage,
} from "./api/client";
import { keywordRanges, type KeywordRanges, visibleKeywordMarks, keywordOwners } from "./keywordDecorations";
import { inheritAt, linkArgsAt, linkAt } from "./typstRefs";
import { formatNoteId, parseNoteId } from "./utils";

/**
 * Completion for Typst notes, whose links and settings name other notes:
 * - in `#lin[…]`, the keywords notes have, each with its note, so a link matches one exactly;
 * - after `#lin(`, the `rel:` of a link that isn't to a prerequisite, then its keywords;
 * - after `inh:` or `mv:` in a cloze's settings, the notes whose id or text matches what is typed,
 *   then that note's card orders, so inheriting or moving a card needs no lookup;
 * - and a `#key[…]` keyword another note already has is underlined, as links to it would find both.
 * Ctrl+N and Ctrl+P pick and Ctrl+Y accepts, as in Vim, besides the arrows and Enter.
 */

// Every keyword, fetched once and again after a while, as notes gain keywords while writing
const KEYWORDS_MAX_AGE_MS = 30_000;
function indexKeywords(list: [number, string][]) {
  return { list, owners: keywordOwners(list) };
}
let keywords: { at: number; list: Promise<ReturnType<typeof indexKeywords>> } | null = null;

function allKeywords() {
  if (!keywords || Date.now() - keywords.at > KEYWORDS_MAX_AGE_MS) {
    const list = listKeywords().then(indexKeywords).catch(() => {
      // Offline or failed: none, and asked again next time
      keywords = null;
      return indexKeywords([]);
    });
    keywords = { at: Date.now(), list };
  }
  return keywords.list;
}

async function linkCompletions(
  context: CompletionContext,
): Promise<CompletionResult | null> {
  const line = context.state.doc.lineAt(context.pos);
  const at = linkAt(line.text.slice(0, context.pos - line.from));
  if (!at) return null;
  const { list } = await allKeywords();
  if (context.aborted) return null;
  // One entry per keyword, naming each note that has it
  const notes = new Map<string, { keyword: string; ids: number[] }>();
  for (const [noteId, keyword] of list) {
    const key = keyword.toLowerCase();
    const entry = notes.get(key) ?? { keyword, ids: [] };
    entry.ids.push(noteId);
    notes.set(key, entry);
  }
  return {
    from: line.from + at.from,
    options: [...notes.values()].map(({ keyword, ids }) => ({
      label: keyword,
      detail: ids.map(formatNoteId).join(", "),
      type: "keyword",
    })),
    validFor: /^[^[\]]*$/,
  };
}

/** Which way a link can point besides to a prerequisite, the default (see `LinkRelation`). */
const LINK_RELATIONS = [
  { rel: "dependent", detail: "the linked note builds on this one" },
  { rel: "related", detail: "neither builds on the other" },
];

function linkRelationCompletions(context: CompletionContext): CompletionResult | null {
  const line = context.state.doc.lineAt(context.pos);
  const at = linkArgsAt(line.text.slice(0, context.pos - line.from));
  if (!at) return null;
  return {
    from: line.from + at.from,
    options: LINK_RELATIONS.map(({ rel, detail }) => {
      const insert = `rel: "${rel}")[]`;
      return {
        label: `rel: "${rel}"`,
        detail,
        type: "property",
        // Closes the arguments, taking the `)` the editor may have closed them with, and goes on
        // to the keyword inside the brackets
        apply: (view: EditorView, _completion: Completion, from: number, to: number) => {
          const end = view.state.sliceDoc(to, to + 1) === ")" ? to + 1 : to;
          view.dispatch({
            changes: { from, to: end, insert },
            selection: { anchor: from + insert.length - 1 },
          });
          startCompletion(view);
        },
      };
    }),
    validFor: /^[^()[\]]*$/,
  };
}

/** A note to inherit from: its id, then its card orders, asked for at once. */
function noteOption(noteId: number, text: string): Completion {
  const id = formatNoteId(noteId);
  return {
    label: id,
    detail: text.replace(/\s+/g, " ").trim().slice(0, 60),
    type: "variable",
    apply: (view, _completion, from, to) => {
      view.dispatch({
        changes: { from, to, insert: `${id}/` },
        selection: { anchor: from + id.length + 1 },
      });
      startCompletion(view);
    },
  };
}

async function inheritCompletions(
  context: CompletionContext,
): Promise<CompletionResult | null> {
  const line = context.state.doc.lineAt(context.pos);
  const at = inheritAt(line.text.slice(0, context.pos - line.from));
  if (!at) return null;
  const from = line.from + at.from;
  if (at.kind === "order") {
    const noteId = parseNoteId(at.noteId);
    if (noteId === null) return null;
    const cards = await getCardsForNote(noteId).catch(() => []);
    if (context.aborted) return null;
    return {
      from,
      options: cards.map((card) => ({
        label: String(card.order),
        detail: `card ${card.id}`,
        type: "constant",
      })),
      validFor: /^\d*$/,
    };
  }
  // Nothing typed yet: wait for the id or some of the note's text
  if (at.text === "" && !context.explicit) return null;
  const words = at.text.replace(/[^\p{L}\p{N}]+/gu, " ").trim();
  const [byId, found] = await Promise.all([
    /^[a-z]{1,6}$/i.test(at.text)
      ? getNote(parseNoteId(at.text) ?? -1).catch(() => null)
      : Promise.resolve(null),
    words
      ? searchNotesPage(words, 1, 8)
          .then((page) => page.items.map((m) => m.note))
          .catch(() => [])
      : Promise.resolve([]),
  ]);
  if (context.aborted) return null;
  const notes = [...(byId ? [byId] : []), ...found.filter((n) => n.id !== byId?.id)];
  return {
    from,
    // Matched by id or text on the server, so not filtered again by the id typed
    filter: false,
    options: notes.map((note) => noteOption(note.id, note.data)),
    // Typed words are looked up again as they change
    validFor: () => false,
  };
}

// Pick and accept as Vim's completion does, where a completion list is open
const vimCompletionKeys = Prec.highest(
  keymap.of(
    [
      ["Ctrl-n", moveCompletionSelection(true)],
      ["Ctrl-p", moveCompletionSelection(false)],
      ["Ctrl-y", acceptCompletion],
    ].map(([key, command]) => ({
      key: key as string,
      run: (view: EditorView) =>
        completionStatus(view.state) === "active" &&
        (command as (view: EditorView) => boolean)(view),
    })),
  ),
);

const keywordsLoaded = StateEffect.define<null>();

/** Underlines each `#key[…]` keyword that a note other than `noteId` already has. */
function duplicateKeywords(noteId: number | null): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet = Decoration.none;
      owners: Map<string, number[]> | null = null;
      ranges: KeywordRanges | null = null;
      destroyed = false;
      constructor(view: EditorView) {
        void allKeywords().then(({ owners }) => {
          if (this.destroyed) return;
          this.owners = owners;
          view.dispatch({ effects: keywordsLoaded.of(null) });
        });
      }
      destroy() {
        this.destroyed = true;
      }
      update(update: ViewUpdate) {
        if (!this.owners) return;
        const loaded = update.transactions.some(t => t.effects.some(e => e.is(keywordsLoaded)));
        if (loaded || update.docChanged) {
          this.ranges = keywordRanges(update.state.doc, this.owners, noteId,
            loaded || !this.ranges ? undefined : { ranges: this.ranges, changes: update.changes });
        }
        if (this.ranges && (loaded || update.docChanged || update.viewportChanged)) {
          const { from, to } = update.view.viewport;
          this.decorations = visibleKeywordMarks(this.ranges, from, to);
        }
      }
    },
    { decorations: (v) => v.decorations },
  );
  const theme = EditorView.baseTheme({
    ".cm-duplicate-keyword": {
      textDecoration: "underline wavy var(--warning)",
      textUnderlineOffset: "3px",
    },
  });
  return [plugin, theme];
}

/** Whether the cursor is where this module completes, where Typst's own names would be noise. */
function oursAt(context: CompletionContext): boolean {
  const line = context.state.doc.lineAt(context.pos);
  const before = line.text.slice(0, context.pos - line.from);
  return linkAt(before) !== null || linkArgsAt(before) !== null || inheritAt(before) !== null;
}

/** The Typst language's own completion (its functions and names), everywhere else. */
async function typstCompletions(
  context: CompletionContext,
): Promise<CompletionResult | null> {
  if (oursAt(context)) return null;
  for (const source of context.state.languageDataAt<CompletionSource>(
    "autocomplete",
    context.pos,
  )) {
    const result = await source(context);
    if (result) return result;
  }
  return null;
}

/** The completion and checks of a Typst note's text, written as note `noteId` (null if new). */
export function typstNoteCompletion(noteId: number | null): Extension {
  return [
    autocompletion({
      override: [linkCompletions, linkRelationCompletions, inheritCompletions, typstCompletions],
    }),
    vimCompletionKeys,
    duplicateKeywords(noteId),
  ];
}
