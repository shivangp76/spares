import { usePageKeys, type PageShortcut } from './usePageKeys';
import type { useRowSelection } from './useRowSelection';

/**
 * The keyboard shortcuts of a list with a detail pane: moving through its rows and pages, and
 * checking them, before the page's own `extra`. Returns the `help` the page's `Navbar` takes.
 */
export function useListKeys({ rows, displayed, openIndex, open, close, edit, prevPage, nextPage, searching, body, search, extra = [] }: {
  rows: ReturnType<typeof useRowSelection>;
  /** The rows on this page. */
  displayed: { id: number }[];
  /** Where in `displayed` the row open in the detail pane is, or -1. */
  openIndex: number;
  /** Opens the `index`th of `displayed` in the detail pane. */
  open: (index: number) => void;
  /** Closes the detail pane, or is null when nothing is open in it. */
  close: (() => void) | null;
  /** What `e` and Enter do to what is open in the detail pane, which is null when nothing is. */
  edit: { description: string; run: (() => void) | null };
  /** Go to the previous or next page, or are null on the first or last. */
  prevPage: (() => void) | null;
  nextPage: (() => void) | null;
  searching: boolean;
  /** The table's body, to scroll the row opened into view. */
  body: React.RefObject<HTMLTableSectionElement | null>;
  /** The search box, which `/` focuses. */
  search: React.RefObject<HTMLInputElement | null>;
  extra?: PageShortcut[];
}) {
  const scope = searching ? 'every row found' : 'the rows on this page';
  const count = displayed.length;

  function show(index: number) {
    open(index);
    requestAnimationFrame(() => body.current?.rows[index]?.scrollIntoView({ block: 'nearest' }));
  }

  /** Opens the row `step` from the open one, wrapping around at either end as the detail pane's
      Previous and Next do. With none open, ↓ opens the first and ↑ the last. */
  function move(step: 1 | -1) {
    show(openIndex < 0 ? (step === 1 ? 0 : count - 1) : (openIndex + step + count) % count);
  }

  /** Opens the row `step` from the open one, checking both, as a shift-click would. It stops at
      either end, as a range doesn't wrap around. */
  function extend(step: 1 | -1) {
    const index = openIndex < 0 ? (step === 1 ? 0 : count - 1) : Math.min(count - 1, Math.max(0, openIndex + step));
    rows.checkRows(displayed, openIndex < 0 ? [index] : [openIndex, index]);
    show(index);
  }

  return usePageKeys([
    { keys: ['↓'], codes: ['ArrowDown'], vim: ['j'], description: 'Open the next row, or with none open, the first', available: count > 0, run: () => move(1) },
    { keys: ['↑'], codes: ['ArrowUp'], vim: ['k'], description: 'Open the previous row, or with none open, the last', available: count > 0, run: () => move(-1) },
    { keys: ['Shift+↓', 'Shift+↑'], codes: ['Shift+ArrowDown', 'Shift+ArrowUp'], description: 'Open the next or previous row, checking it and the open one', available: count > 0, run: code => extend(code === 'Shift+ArrowDown' ? 1 : -1) },
    { keys: ['←'], codes: ['ArrowLeft'], description: 'Go to the previous page', available: prevPage !== null, run: () => prevPage?.() },
    { keys: ['→'], codes: ['ArrowRight'], description: 'Go to the next page', available: nextPage !== null, run: () => nextPage?.() },
    { keys: ['Esc'], codes: ['Escape'], description: 'Close what is open in the detail pane', available: close !== null, run: () => close?.() },
    { keys: ['e', 'Enter'], codes: ['e', 'Enter'], description: edit.description, available: edit.run !== null, run: () => edit.run?.() },
    { keys: ['/'], codes: ['/'], description: 'Go to the search box', available: true, run: () => search.current?.select() },
    { keys: ['x'], codes: ['x'], description: 'Check or uncheck the open row', available: openIndex >= 0, run: () => rows.toggleRow(displayed, openIndex, false) },
    // Ctrl+A only stands for `a`, which it would otherwise select the page's text with
    { keys: ['a', 'Ctrl+A'], codes: ['a', 'Ctrl+a'], description: `Check ${scope} (Cmd+A on a Mac)`, available: count > 0, run: () => rows.selectAll(displayed) },
    { keys: ['n'], codes: ['n'], description: 'Uncheck every row', available: true, run: rows.selectNone },
    { keys: ['i'], codes: ['i'], description: `Invert which of ${scope} are checked`, available: count > 0, run: () => rows.invert(displayed) },
    ...extra,
  ]);
}
