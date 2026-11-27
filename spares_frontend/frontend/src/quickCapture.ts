/**
 * The tags a note written with quick capture (`c`, `components/QuickCapture.tsx`) starts with,
 * from what the page shows: the tag highlighted on the Tags page, the tag the Notes page searches
 * for, or the card being reviewed's note's tags. A page sets them while it shows them, and clears
 * them when it stops.
 */
import { useEffect } from 'react';
import type { NewNoteStart } from './components/NewNoteForm';

let current: string[] = [];

/** The tags of what the page shows now, for a new note to start with. */
export function captureTags(): string[] {
  return current;
}

/** Offers `tags` to quick capture while the calling page shows them. */
export function useCaptureTags(tags: readonly string[] | null | undefined): void {
  const key = JSON.stringify(tags ?? []);
  useEffect(() => {
    current = JSON.parse(key) as string[];
    return () => {
      current = [];
    };
  }, [key]);
}

/** The tag a search finds the notes of, if it is only that: `tag=x` or `tag="x"`. */
export function searchedTag(query: string | null | undefined): string | null {
  const match = /^\s*tag\s*=\s*(?:"((?:[^"\\]|\\.)*)"|([^\s"()]+))\s*$/.exec(query ?? '');
  if (!match) return null;
  return match[1] !== undefined ? match[1].replace(/\\(.)/g, '$1') : match[2];
}

/** The event that opens quick capture with a note to start from (`captureNote`). */
export const CAPTURE_EVENT = 'spares-capture';

/** Opens quick capture with a new note starting as `start`, such as a card for a word looked up. */
export function captureNote(start: NewNoteStart): void {
  window.dispatchEvent(new CustomEvent<NewNoteStart>(CAPTURE_EVENT, { detail: start }));
}
