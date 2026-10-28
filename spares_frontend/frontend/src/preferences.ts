import { useMemo, useSyncExternalStore } from 'react';

const CHANGE_EVENT = 'spares-preferences-change';

/** A setting kept in this browser, falling back to `defaultValue` when unset or without storage. */
function booleanPreference(key: string, defaultValue: () => boolean) {
  function get(): boolean {
    try {
      const stored = localStorage.getItem(key);
      return stored === null ? defaultValue() : stored === 'true';
    } catch {
      // Without storage the preference can't be remembered, so the default is used
      return defaultValue();
    }
  }

  function set(value: boolean): void {
    try {
      localStorage.setItem(key, String(value));
    } catch {
      // Without storage the preference can't be changed, so the default stays
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }

  function use(): boolean {
    return useSyncExternalStore(subscribe, get);
  }

  return { get, set, use };
}

/** A setting kept in this browser that is one of `values`, falling back to `defaultValue`. */
function choicePreference<T extends string>(key: string, values: readonly T[], defaultValue: T) {
  function get(): T {
    try {
      const stored = localStorage.getItem(key);
      return values.find(v => v === stored) ?? defaultValue;
    } catch {
      return defaultValue;
    }
  }

  function set(value: T): void {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Without storage the preference can't be changed, so the default stays
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }

  function use(): T {
    return useSyncExternalStore(subscribe, get);
  }

  return { get, set, use };
}

/** A setting kept in this browser that is some of `values`, kept in their order, falling back to
    `defaultValue` when unset or unreadable. Values no longer known are dropped. */
export function listPreference<T extends string>(key: string, values: readonly T[], defaultValue: readonly T[]) {
  function stored(): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  function parse(raw: string | null): T[] {
    if (raw === null) return [...defaultValue];
    try {
      const list: unknown = JSON.parse(raw);
      if (!Array.isArray(list)) return [...defaultValue];
      return values.filter(v => list.includes(v));
    } catch {
      return [...defaultValue];
    }
  }

  function get(): T[] {
    return parse(stored());
  }

  function set(value: readonly T[]): void {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Without storage the preference can't be changed, so the default stays
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }

  function use(): T[] {
    // The stored string is compared, as a parsed list would be new on each read
    const raw = useSyncExternalStore(subscribe, stored);
    return useMemo(() => parse(raw), [raw]);
  }

  return { get, set, use };
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, onChange);
  return () => window.removeEventListener(CHANGE_EVENT, onChange);
}

/** Whether the review page shows the running recall and rate timers. Shown unless turned off. */
const showReviewTimer = booleanPreference('spares_show_review_timer', () => true);
export const setShowReviewTimer = showReviewTimer.set;
export const useShowReviewTimer = showReviewTimer.use;

/** Whether the review page reads cards' text aloud (see `spares_core/src/parsers/tts.rs`). Audio
    clips are played either way. On unless turned off. */
const readAloud = booleanPreference('spares_read_aloud', () => true);
export const setReadAloud = readAloud.set;
export const useReadAloud = readAloud.use;

/** Whether a note's local graph, of the notes it links to and that link to it, is shown open. */
const localGraphOpen = booleanPreference('spares_local_graph_open', () => false);
export const setLocalGraphOpen = localGraphOpen.set;
export const useLocalGraphOpen = localGraphOpen.use;

/** Whether note editors use Vim keybindings. Off by default on touchscreens, where a phone
    keyboard can't leave Vim's normal mode to type. */
const vimKeybindings = booleanPreference('spares_vim_keybindings', () => !window.matchMedia('(pointer: coarse)').matches);
export const setVimKeybindings = vimKeybindings.set;
export const useVimKeybindings = vimKeybindings.use;

export const EDITOR_FONT_SIZES = ['12', '13', '14', '16', '18', '20'] as const;
export type EditorFontSize = typeof EDITOR_FONT_SIZES[number];
/** How large the note editors' text is, in pixels. The typed answer box keeps its own size. */
const editorFontSize = choicePreference<EditorFontSize>('spares_editor_font_size', EDITOR_FONT_SIZES, '13');
export const setEditorFontSize = editorFontSize.set;
export const useEditorFontSize = editorFontSize.use;

export const EDITOR_FONTS = ['monospace', 'interface'] as const;
export type EditorFont = typeof EDITOR_FONTS[number];
export const EDITOR_FONT_LABELS: Record<EditorFont, string> = { monospace: 'Monospace', interface: 'Same as Interface' };
/** Which font the note editors use, as notes of prose can be easier to read in the interface's. */
const editorFont = choicePreference<EditorFont>('spares_editor_font', EDITOR_FONTS, 'monospace');
export const setEditorFont = editorFont.set;
export const useEditorFont = editorFont.use;

export const LINE_WRAPPINGS = ['phones', 'always', 'never'] as const;
export type LineWrapping = typeof LINE_WRAPPINGS[number];
export const LINE_WRAPPING_LABELS: Record<LineWrapping, string> = { phones: 'On Phones', always: 'Always', never: 'Never' };
/** When the note editors wrap long lines rather than scrolling sideways. */
const lineWrapping = choicePreference<LineWrapping>('spares_editor_line_wrapping', LINE_WRAPPINGS, 'phones');
export const setLineWrapping = lineWrapping.set;
export const useLineWrapping = lineWrapping.use;

/** Whether the note editor shows line numbers beside the note's text. Never on phones, which need
    the width for the text. */
const editorLineNumbers = booleanPreference('spares_editor_line_numbers', () => true);
export const setEditorLineNumbers = editorLineNumbers.set;
export const useEditorLineNumbers = editorLineNumbers.use;

/** Whether the browser marks misspelled words in the note's text. Off unless turned on, as notes of
    code or Typst markup would be full of marks. */
const editorSpellcheck = booleanPreference('spares_editor_spellcheck', () => false);
export const setEditorSpellcheck = editorSpellcheck.set;
export const useEditorSpellcheck = editorSpellcheck.use;

/** Whether the note editors type the closing bracket or quote along with the opening one. */
const closeBrackets = booleanPreference('spares_editor_close_brackets', () => true);
export const setCloseBrackets = closeBrackets.set;
export const useCloseBrackets = closeBrackets.use;

export const PAGE_WIDTHS = ['normal', 'wide', 'full'] as const;
export type PageWidth = typeof PAGE_WIDTHS[number];
export const PAGE_WIDTH_LABELS: Record<PageWidth, string> = { normal: 'Normal', wide: 'Wide', full: 'Full Screen' };
/** How wide pages of a single column, such as the review page, are on large screens. Pages with
    a list beside a detail panel already use the full width. Phones use their full width whatever
    it is. Also read by the inline script in index.html, which applies it before the first paint. */
const pageWidth = choicePreference<PageWidth>('spares_page_width', PAGE_WIDTHS, 'normal');
export const usePageWidth = pageWidth.use;

/** Sets `data-page-width` on the root element, which index.css uses to pick `--page-max-width`. */
export function setPageWidth(width: PageWidth): void {
  pageWidth.set(width);
  document.documentElement.dataset.pageWidth = width;
}

export const CARD_ZOOMS = ['100', '125', '150', '175', '200'] as const;
export type CardZoom = typeof CARD_ZOOMS[number];
/** How large cards' text is, in percent, for reading from further away on large screens. */
const cardZoom = choicePreference<CardZoom>('spares_card_zoom', CARD_ZOOMS, '100');
export const setCardZoom = cardZoom.set;
export const useCardZoom = cardZoom.use;

export const PAGES_PER_ROW = ['1', '2', 'auto'] as const;
export type PagesPerRow = typeof PAGES_PER_ROW[number];
export const PAGES_PER_ROW_LABELS: Record<PagesPerRow, string> = { 1: 'One', 2: 'Two', auto: 'Two When Wide' };
/** How many pages of a multi-page Typst card are shown side by side, wherever cards are shown. */
const pagesPerRow = choicePreference<PagesPerRow>('spares_pages_per_row', PAGES_PER_ROW, 'auto');
export const setPagesPerRow = pagesPerRow.set;
export const usePagesPerRow = pagesPerRow.use;

export const PAGE_SIZES = ['10', '20', '50', '100', '200'] as const;
export type PageSize = typeof PAGE_SIZES[number];
/** How many notes the notes page lists on each page. */
const notesPageSize = choicePreference<PageSize>('spares_notes_page_size', PAGE_SIZES, '20');
export const setNotesPageSize = notesPageSize.set;
export const useNotesPageSize = notesPageSize.use;

/** How many cards the cards page lists on each page. */
const cardsPageSize = choicePreference<PageSize>('spares_cards_page_size', PAGE_SIZES, '20');
export const setCardsPageSize = cardsPageSize.set;
export const useCardsPageSize = cardsPageSize.use;

/** How many tags the tags page lists on each page. */
const tagsPageSize = choicePreference<PageSize>('spares_tags_page_size', PAGE_SIZES, '50');
export const setTagsPageSize = tagsPageSize.set;
export const useTagsPageSize = tagsPageSize.use;

/** How many rows the keywords page lists on each page of a tab. */
const keywordsPageSize = choicePreference<PageSize>('spares_keywords_page_size', PAGE_SIZES, '50');
export const setKeywordsPageSize = keywordsPageSize.set;
export const useKeywordsPageSize = keywordsPageSize.use;

export const CARD_COLUMNS = ['id', 'note', 'text', 'tags', 'order', 'state', 'due', 'queue', 'retention', 'special', 'stability', 'difficulty', 'retrievability', 'interval', 'reps', 'lapses', 'leech', 'reviewed', 'created', 'updated'] as const;
export type CardColumn = typeof CARD_COLUMNS[number];
/** Which columns the cards page's table shows. */
const cardColumns = listPreference<CardColumn>('spares_cards_columns', CARD_COLUMNS, ['id', 'note', 'text', 'order', 'state', 'due', 'retention', 'special']);
export const setCardColumns = cardColumns.set;
export const useCardColumns = cardColumns.use;

export const NOTE_COLUMNS = ['id', 'data', 'tags', 'keywords', 'cards', 'parser', 'created', 'updated'] as const;
export type NoteColumn = typeof NOTE_COLUMNS[number];
/** Which columns the notes page's table shows. */
const noteColumns = listPreference<NoteColumn>('spares_notes_columns', NOTE_COLUMNS, ['id', 'data', 'tags', 'keywords', 'cards']);
export const setNoteColumns = noteColumns.set;
export const useNoteColumns = noteColumns.use;
