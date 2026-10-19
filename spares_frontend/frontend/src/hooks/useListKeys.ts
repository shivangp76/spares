import { useEffect, useState } from 'react';
import type { Keybinding } from '../help/shortcuts';
import type { useRowSelection } from './useRowSelection';

/** Whether `target` takes the keys pressed in it, e.g. to type them, or to seek with the arrows.
    Checkboxes take no keys but Space, so the list's keys still work after one is clicked. */
function takesKeys(target: EventTarget | null): boolean {
  if (target instanceof HTMLInputElement) return target.type !== 'checkbox' && target.type !== 'radio';
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || target instanceof HTMLMediaElement) return true;
  return target instanceof Element && target.closest('.cm-editor, [contenteditable="true"]') !== null;
}

/**
 * The keyboard shortcuts of a list with a detail pane: moving through its rows and pages, and
 * checking them. `?` opens the page's help, which lists them. Returns the `help` the page's
 * `Navbar` takes.
 */
export function useListKeys({ rows, displayed, openIndex, open, prevPage, nextPage, searching, body }: {
  rows: ReturnType<typeof useRowSelection>;
  /** The rows on this page. */
  displayed: { id: number }[];
  /** Where in `displayed` the row open in the detail pane is, or -1. */
  openIndex: number;
  /** Opens the `index`th of `displayed` in the detail pane. */
  open: (index: number) => void;
  /** Go to the previous or next page, or are null on the first or last. */
  prevPage: (() => void) | null;
  nextPage: (() => void) | null;
  searching: boolean;
  /** The table's body, to scroll the row opened into view. */
  body: React.RefObject<HTMLTableSectionElement | null>;
}) {
  const [helpOpen, setHelpOpen] = useState(false);
  const scope = searching ? 'every row found' : 'the rows on this page';
  const count = displayed.length;

  /** Opens the row `step` from the open one, wrapping around at either end as the detail pane's
      Previous and Next do. With none open, ↓ opens the first and ↑ the last. */
  function move(step: 1 | -1) {
    const index = openIndex < 0 ? (step === 1 ? 0 : count - 1) : (openIndex + step + count) % count;
    open(index);
    requestAnimationFrame(() => body.current?.rows[index]?.scrollIntoView({ block: 'nearest' }));
  }

  // `codes` are the `KeyboardEvent.key`s of `keys`, which are how the help shows them
  const shortcuts: (Keybinding & { codes: string[]; run: () => void })[] = [
    { keys: ['↓'], codes: ['ArrowDown'], description: 'Open the next row, or with none open, the first', available: count > 0, run: () => move(1) },
    { keys: ['↑'], codes: ['ArrowUp'], description: 'Open the previous row, or with none open, the last', available: count > 0, run: () => move(-1) },
    { keys: ['←'], codes: ['ArrowLeft'], description: 'Go to the previous page', available: prevPage !== null, run: () => prevPage?.() },
    { keys: ['→'], codes: ['ArrowRight'], description: 'Go to the next page', available: nextPage !== null, run: () => nextPage?.() },
    { keys: ['x'], codes: ['x'], description: 'Check or uncheck the open row', available: openIndex >= 0, run: () => rows.toggleRow(displayed, openIndex, false) },
    // Ctrl+A only stands for `a`, which it would otherwise select the page's text with
    { keys: ['a', 'Ctrl+A'], codes: ['a'], description: `Check ${scope} (Cmd+A on a Mac)`, available: count > 0, run: () => rows.selectAll(displayed) },
    { keys: ['n'], codes: ['n'], description: 'Uncheck every row', available: true, run: rows.selectNone },
    { keys: ['i'], codes: ['i'], description: `Invert which of ${scope} are checked`, available: count > 0, run: () => rows.invert(displayed) },
  ];

  // Re-bound on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (takesKeys(e.target)) return;
      const selectAllKey = e.key === 'a' && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey;
      if (!selectAllKey && (e.ctrlKey || e.metaKey || e.altKey)) return;
      if (e.key === '?') {
        e.preventDefault();
        setHelpOpen(open => !open);
        return;
      }
      // The help covers the list, so nothing should change behind it
      if (helpOpen) return;
      // Shift+arrows are left to the browser, which selects text with them
      if (e.shiftKey && e.key.startsWith('Arrow')) return;
      const shortcut = shortcuts.find(s => s.codes.includes(e.key));
      if (!shortcut?.available) return;
      e.preventDefault();
      shortcut.run();
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });

  const bindings: Keybinding[] = shortcuts.map(({ keys, description, available }) => ({ keys, description, available }));
  return { open: helpOpen, onOpenChange: setHelpOpen, bindings };
}
