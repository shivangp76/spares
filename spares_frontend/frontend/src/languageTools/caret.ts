/**
 * The text at a point of a card's document, for looking it up: the character under the point, and
 * the text after it in the same block, leaving out readings (`<rt>`), with the text nodes it came
 * from so that the matched part can be highlighted. The card's document is in an iframe the app
 * can reach (`HtmlViewer`), but which runs no scripts of its own.
 */
import { MAX_CHARS, lookupStart } from './text';

interface Segment {
  node: Text;
  start: number;
  end: number;
}

export interface TextAt {
  text: string;
  segments: Segment[];
}

const BLOCK = 'p, li, dd, dt, td, th, h1, h2, h3, h4, h5, h6, blockquote, figcaption, pre, div, body';

function inReading(node: Node): boolean {
  return !!node.parentElement?.closest('rt, rp');
}

/** The text node and offset under the point (x, y) of `doc`'s viewport. */
function caretAt(doc: Document, x: number, y: number): { node: Node; offset: number } | null {
  if ('caretPositionFromPoint' in doc) {
    const position = (
      doc as Document & {
        caretPositionFromPoint(x: number, y: number): { offsetNode: Node; offset: number } | null;
      }
    ).caretPositionFromPoint(x, y);
    if (position) return { node: position.offsetNode, offset: position.offset };
  }
  // WebKit
  const range = (doc as Document & { caretRangeFromPoint?(x: number, y: number): Range | null }).caretRangeFromPoint?.(
    x,
    y,
  );
  return range ? { node: range.startContainer, offset: range.startOffset } : null;
}

/** Whether the character of `node` at `offset` is drawn under (x, y). */
function charContains(doc: Document, node: Text, offset: number, x: number, y: number): boolean {
  if (offset >= node.length) return false;
  const range = doc.createRange();
  range.setStart(node, offset);
  range.setEnd(node, offset + 1);
  return [...range.getClientRects()].some(r => x >= r.left && x <= r.right && y >= r.top && y <= r.bottom);
}

/** The text from `node` at `offset` on, in its block, without readings. */
export function textFrom(doc: Document, node: Text, offset: number): TextAt {
  const block = node.parentElement?.closest(BLOCK) ?? doc.body;
  const walker = doc.createTreeWalker(block, NodeFilter.SHOW_TEXT);
  walker.currentNode = node;
  const segments: Segment[] = [];
  let text = '';
  let current: Node | null = node;
  let start = offset;
  while (current && [...text].length < MAX_CHARS) {
    if (!inReading(current)) {
      const value = (current as Text).data;
      const end = Math.min(value.length, start + MAX_CHARS * 2);
      segments.push({ node: current as Text, start, end });
      text += value.slice(start, end);
    }
    current = walker.nextNode();
    start = 0;
  }
  const chars = [...text].slice(0, MAX_CHARS).join('');
  return { text: chars, segments };
}

/** The text to look up at (x, y) in `doc`, or null if no text is there. */
export function textAtPoint(doc: Document, x: number, y: number): TextAt | null {
  const caret = caretAt(doc, x, y);
  if (!caret || caret.node.nodeType !== Node.TEXT_NODE) return null;
  let node = caret.node as Text;
  let offset = caret.offset;
  // The caret goes to the nearest boundary, which is after the character on its right half
  if (!charContains(doc, node, offset, x, y) && offset > 0 && charContains(doc, node, offset - 1, x, y)) offset -= 1;
  if (!charContains(doc, node, offset, x, y)) return null;
  if (inReading(node)) {
    // A reading looks up its base text
    const ruby = node.parentElement?.closest('ruby');
    const walker = ruby && doc.createTreeWalker(ruby, NodeFilter.SHOW_TEXT);
    let base = walker?.nextNode() ?? null;
    while (base && inReading(base)) base = walker!.nextNode();
    if (!base) return null;
    node = base as Text;
    offset = 0;
  } else {
    offset = lookupStart(node.data, offset);
  }
  const at = textFrom(doc, node, offset);
  return at.text.trim() ? at : null;
}

/** The ranges of the first `chars` characters of `at`, for highlighting them. */
export function rangesOf(doc: Document, at: TextAt, chars: number): Range[] {
  const ranges: Range[] = [];
  let left = chars;
  for (const segment of at.segments) {
    if (left <= 0) break;
    const value = segment.node.data.slice(segment.start, segment.end);
    const taken = [...value].slice(0, left).join('');
    if (taken.length === 0) continue;
    const range = doc.createRange();
    range.setStart(segment.node, segment.start);
    range.setEnd(segment.node, segment.start + taken.length);
    ranges.push(range);
    left -= [...taken].length;
  }
  return ranges;
}

const HIGHLIGHT = 'spares-lookup';

/** Highlights `ranges` in `doc` with the CSS Highlight API, which leaves the DOM as it is. */
export function highlight(doc: Document, ranges: Range[]): void {
  const win = doc.defaultView as (Window & typeof globalThis) | null;
  if (!win || !('highlights' in win.CSS)) return;
  if (!doc.getElementById(HIGHLIGHT)) {
    const style = doc.createElement('style');
    style.id = HIGHLIGHT;
    const accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#3b82f6';
    style.textContent = `::highlight(${HIGHLIGHT}) { background-color: color-mix(in srgb, ${accent} 28%, transparent); }`;
    doc.head?.append(style);
  }
  if (ranges.length === 0) win.CSS.highlights.delete(HIGHLIGHT);
  else win.CSS.highlights.set(HIGHLIGHT, new win.Highlight(...ranges));
}

/** Where `ranges` are, in the app's viewport, from the iframe they are in. */
export function rectOf(doc: Document, ranges: Range[]): DOMRect | null {
  const frame = doc.defaultView?.frameElement?.getBoundingClientRect();
  const first = ranges[0]?.getBoundingClientRect();
  if (!frame || !first) return null;
  return new DOMRect(frame.left + first.left, frame.top + first.top, first.width, first.height);
}
