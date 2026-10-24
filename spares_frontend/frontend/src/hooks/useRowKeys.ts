import { useState } from 'react';
import { usePageKeys } from './usePageKeys';

/**
 * The keyboard shortcuts of a table without a detail pane, whose rows are highlighted one at a time
 * for the keys that act on them. Returns the highlighted row, or -1, and the `help` the page's
 * `Navbar` takes.
 */
export function useRowKeys({ count, body, edit, remove, filter, prevPage = null, nextPage = null }: {
  /** How many rows the table has. */
  count: number;
  /** The table's body, to scroll the row highlighted into view. */
  body: React.RefObject<HTMLTableSectionElement | null>;
  /** What `e` and Enter do to the highlighted row. */
  edit: { description: string; run: (index: number) => void };
  /** What Delete does to it, which should ask first. */
  remove: { description: string; run: (index: number) => void };
  /** The box that filters the rows, which `/` focuses. */
  filter?: React.RefObject<HTMLInputElement | null>;
  /** Go to the previous or next page, or are null on the first or last, or without pages. */
  prevPage?: (() => void) | null;
  nextPage?: (() => void) | null;
}) {
  const [highlighted, setHighlighted] = useState(-1);
  // Kept where it was when rows are removed, so it moves to the next
  const index = Math.min(highlighted, count - 1);

  /** Highlights the row `step` from the highlighted one, wrapping around at either end. */
  function move(step: 1 | -1) {
    const next = index < 0 ? (step === 1 ? 0 : count - 1) : (index + step + count) % count;
    setHighlighted(next);
    requestAnimationFrame(() => body.current?.rows[next]?.scrollIntoView({ block: 'nearest' }));
  }

  const help = usePageKeys([
    { keys: ['↓'], codes: ['ArrowDown'], vim: ['j'], description: 'Highlight the next row, or with none highlighted, the first', available: count > 0, run: () => move(1) },
    { keys: ['↑'], codes: ['ArrowUp'], vim: ['k'], description: 'Highlight the previous row, or with none highlighted, the last', available: count > 0, run: () => move(-1) },
    ...(prevPage || nextPage ? [
      { keys: ['←'], codes: ['ArrowLeft'], description: 'Go to the previous page', available: prevPage !== null, run: () => prevPage?.() },
      { keys: ['→'], codes: ['ArrowRight'], description: 'Go to the next page', available: nextPage !== null, run: () => nextPage?.() },
    ] : []),
    { keys: ['e', 'Enter'], codes: ['e', 'Enter'], description: edit.description, available: index >= 0, run: () => edit.run(index) },
    // A Mac's Delete key is Backspace
    { keys: ['Delete', '#'], codes: ['Delete', 'Backspace', '#'], description: `${remove.description} (Backspace on a Mac)`, available: index >= 0, run: () => remove.run(index) },
    { keys: ['Esc'], codes: ['Escape'], description: 'Stop highlighting the row', available: index >= 0, run: () => setHighlighted(-1) },
    ...(filter ? [{ keys: ['/'], codes: ['/'], description: 'Go to the filter box', available: true, run: () => filter.current?.select() }] : []),
  ]);
  return { highlighted: index, help };
}
