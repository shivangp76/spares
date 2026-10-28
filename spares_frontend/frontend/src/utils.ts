import type { EditorView } from '@uiw/react-codemirror';
import type { GetReviewCardResponse } from './types/spares';

/** What to show for something thrown, without the `Error: ` that `String(e)` puts before an error's message. */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Saves `text` as a file named `name`, as the browser saves downloads. */
export function downloadText(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

/** Tags as a message lists them. */
export function tagList(tags: string[]): string {
  return tags.map(t => `\`${t}\``).join(', ');
}

/** The tag sync puts on notes that need a look (see `spares sync conflicts`). */
export const SYNC_CONFLICT_TAG = 'sync-conflict';

/** Whether a note's text comes from a file on the laptop (`spares import --live`). */
export function isLiveNote(note: { custom_data: Record<string, unknown> }): boolean {
  return typeof note.custom_data.live_sync_name === 'string';
}

/** The note to show once `updated` is saved: `updated` if it is still the one shown. A note's pane
    hands its update over only after rendering it, by which time the pane may have been closed or
    moved to another note, which showing `updated` would undo. */
export function keepShown<T extends { id: number }>(shown: T | null, updated: T): T | null {
  return shown?.id === updated.id ? updated : shown;
}

/** Whether `note` was changed after `card` was fetched, e.g. by `spares import` of its file. */
export function noteChangedSince(card: { note_updated_at?: string }, note: { updated_at: string }): boolean {
  return card.note_updated_at !== undefined && Date.parse(note.updated_at) > Date.parse(card.note_updated_at);
}

const NOTE_ID_PLACES = 8;
const NOTE_ID_RADIX = 27;

/**
 * A note id's letters, e.g. `gir`. The API sends note ids as their letters read as a base-27
 * number left-aligned to 8 places (`a` = 1 … `z` = 26, an empty place = 0), so that sorting the
 * numbers sorts the letters. Division is used rather than bit shifts, which work on 32 bits and
 * would corrupt these 39-bit values. A value that is not a note id is shown as `#<number>`.
 */
export function formatNoteId(id: number): string {
  const places: number[] = [];
  let rest = id;
  for (let i = 0; i < NOTE_ID_PLACES; i++) {
    places.unshift(rest % NOTE_ID_RADIX);
    rest = Math.floor(rest / NOTE_ID_RADIX);
  }
  const length = places.indexOf(0) === -1 ? NOTE_ID_PLACES : places.indexOf(0);
  const valid = Number.isInteger(id) && rest === 0 && length > 0 && places.slice(length).every(p => p === 0);
  if (!valid) return `#${id}`;
  return places.slice(0, length).map(p => String.fromCharCode(96 + p)).join('');
}

/** The number the API uses for a note id written as 1 to 8 letters, in any case, or `null`. */
export function parseNoteId(letters: string): number | null {
  const s = letters.trim().toLowerCase();
  if (!/^[a-z]{1,8}$/.test(s)) return null;
  let id = 0;
  for (let i = 0; i < NOTE_ID_PLACES; i++) {
    id = id * NOTE_ID_RADIX + (i < s.length ? s.charCodeAt(i) - 96 : 0);
  }
  return id;
}

/** Port of the CLI's `format_duration`, e.g. `7d 0h 0m 0s` or `1m 30s`. */
export function formatDuration(totalSeconds: number): string {
  const days = Math.floor(totalSeconds / 86400);
  const hours = Math.floor((totalSeconds % 86400) / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0 || parts.length) parts.push(`${hours}h`);
  if (minutes > 0 || parts.length) parts.push(`${minutes}m`);
  parts.push(`${seconds}s`);
  return parts.join(' ');
}

/** A scheduling interval at the coarsest useful unit, like Anki's card info, e.g. `10m` or `3.2mo`. */
export function formatInterval(totalSeconds: number): string {
  const units: [number, string][] = [[365 * 86400, 'y'], [30 * 86400, 'mo'], [86400, 'd'], [3600, 'h'], [60, 'm']];
  for (const [size, unit] of units) {
    if (totalSeconds >= size) {
      const value = totalSeconds / size;
      return `${Number.isInteger(value) || value >= 10 ? Math.round(value) : value.toFixed(1)}${unit}`;
    }
  }
  return `${totalSeconds}s`;
}

export const sectionLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 };
export const th: React.CSSProperties = { textAlign: 'left', padding: '8px 12px', borderBottom: '1px solid var(--border-strong)' };
export const td: React.CSSProperties = { padding: '8px 12px', borderBottom: '1px solid var(--border-subtle)' };

/** The rendered file shown as a card's back: its own back, or the whole note. */
export function backPath(card: GetReviewCardResponse): string {
  return backPathOf(card.card_back_rendered_path);
}

/** The HTML shown as a card's back instead of its rendered file, if it has one. */
export function backWebPath(card: GetReviewCardResponse): string | undefined {
  return card.card_back_web_path && backPathOf(card.card_back_web_path);
}

function backPathOf(b: { CardBack: string } | { Note: string }): string {
  return 'CardBack' in b ? b.CardBack : b.Note;
}

/** Sets or removes the search `q` in `params`, keeping the page to return to when it is cleared. */
export function withSearch(params: URLSearchParams, q: string | null): URLSearchParams {
  const next = new URLSearchParams(params);
  if (q === null) next.delete('q');
  else next.set('q', q);
  return next;
}

/** `params` with the results page set, or removed for `null`, keeping the search. */
export function withPage(params: URLSearchParams, page: number | null): URLSearchParams {
  const next = new URLSearchParams(params);
  if (page === null || page <= 1) next.delete('page');
  else next.set('page', String(page));
  return next;
}

/** A part of a preview, which a search matched if `mark` is set. */
export interface PreviewPart {
  text: string;
  mark: boolean;
}

/**
 * A preview of `text` at most `maxLength` long, in parts that `matches` (`[start, end)` string
 * indices, in order) mark or not, with `…` where text is left out. It starts at the beginning,
 * unless that leaves out the first match, when it starts a little before it.
 */
export function highlightPreview(text: string, matches: [number, number][], maxLength: number): PreviewPart[] {
  // Not between the two halves of a character like an emoji
  const boundary = (i: number) => (i > 0 && i < text.length && /[\uDC00-\uDFFF]/.test(text[i]) ? i - 1 : i);
  const first = matches[0];
  const start = first && first[1] > maxLength ? boundary(Math.max(0, first[0] - 30)) : 0;
  const end = boundary(Math.min(text.length, start + maxLength));
  const parts: PreviewPart[] = start > 0 ? [{ text: '…', mark: false }] : [];
  let at = start;
  for (const [matchStart, matchEnd] of matches) {
    if (matchEnd <= at) continue;
    if (matchStart >= end) break;
    const from = Math.max(matchStart, at);
    if (from > at) parts.push({ text: text.slice(at, from), mark: false });
    at = Math.min(matchEnd, end);
    parts.push({ text: text.slice(from, at), mark: true });
  }
  if (at < end) parts.push({ text: text.slice(at, end), mark: false });
  if (end < text.length) parts.push({ text: '…', mark: false });
  return parts;
}

/** Inserts a block of text at the editor's cursor, on its own line, and returns the editor's new
    contents. Without an editor, the block is appended to `value`. */
export function insertBlock(view: EditorView | undefined, value: string, block: string): string {
  const text = block.endsWith('\n') ? block : `${block}\n`;
  if (!view) return value === '' || value.endsWith('\n') ? value + text : `${value}\n${text}`;
  const { doc, selection } = view.state;
  const line = doc.lineAt(selection.main.head);
  // Insert after the cursor's line so the block does not split it
  const from = line.length === 0 ? line.from : line.to;
  const insert = line.length === 0 ? text : `\n${text.replace(/\n$/, '')}`;
  view.dispatch({ changes: { from, insert }, selection: { anchor: from + insert.length } });
  view.focus();
  return view.state.doc.toString();
}

/** Inserts text at the editor's cursor, in place of the selection, and returns the editor's new
    contents. Without an editor, the text is appended to `value` after a space. */
export function insertInline(view: EditorView | undefined, value: string, text: string): string {
  if (!view) return value === '' || /\s$/.test(value) ? value + text : `${value} ${text}`;
  const { from, to } = view.state.selection.main;
  view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: from + text.length } });
  view.focus();
  return view.state.doc.toString();
}

/** Inserts an example of a note's syntax: on lines of its own if it has several, else at the
    cursor. Returns the editor's new contents, as `insertBlock` and `insertInline` do. */
export function insertSource(view: EditorView | undefined, value: string, source: string): string {
  return source.includes('\n') ? insertBlock(view, value, source) : insertInline(view, value, source);
}

/** Extensions of the images cards can show, as the server accepts them. */
const IMAGE_EXTENSIONS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'];
const IMAGE_TYPE_EXTENSIONS: Record<string, string> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/bmp': 'bmp',
  'image/svg+xml': 'svg',
};

/** The name to upload an image file under, or `null` if cards can't show its format. Pasted images
    often have a generic name or none, so without a supported extension, it comes from the type. */
export function imageUploadName(file: { name: string; type: string }): string | null {
  const match = /^(.*)\.([^.]*)$/.exec(file.name);
  if (match && IMAGE_EXTENSIONS.includes(match[2].toLowerCase())) return file.name;
  const extension = IMAGE_TYPE_EXTENSIONS[file.type];
  if (!extension) return null;
  return `${(match ? match[1] : file.name) || 'image'}.${extension}`;
}

/** The URL of the image in HTML dragged from a web page, which is all some browsers hand over for
    it. `null` if the HTML has no image with an http, https or data URL. */
export function draggedImageUrl(html: string): string | null {
  const match = /<img\b[^>]*?\ssrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(html);
  const src = match ? (match[1] ?? match[2] ?? match[3]).replace(/&amp;/g, '&') : null;
  if (!src) return null;
  try {
    return ['http:', 'https:', 'data:'].includes(new URL(src).protocol) ? src : null;
  } catch {
    return null;
  }
}

// Made once, as building a formatter for every table cell is slow. The date is as
// `toLocaleDateString` writes it.
const dateFormat = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'numeric', day: 'numeric' });
const dateTimeFormat = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit' });

/** `at` as a date in the browser's time zone, or a dash without one. */
export function formatDate(at: string | null | undefined): string {
  return at ? dateFormat.format(new Date(at)) : '—';
}

/** `at` as a date and time to the minute in the browser's time zone, or a dash without one. */
export function formatDateTime(at: string | null | undefined): string {
  return at ? dateTimeFormat.format(new Date(at)) : '—';
}
