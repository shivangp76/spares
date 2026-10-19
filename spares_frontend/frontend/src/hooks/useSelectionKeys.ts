import { useEffect, useState } from 'react';
import type { Keybinding } from '../help/shortcuts';
import type { useRowSelection } from './useRowSelection';

/**
 * The keyboard shortcuts of a list's checkboxes, and `?` for the page's help, which lists them.
 * Returns the `help` the page's `Navbar` takes.
 */
export function useSelectionKeys({ rows, displayed, openIndex, searching }: {
  rows: ReturnType<typeof useRowSelection>;
  /** The rows on this page. */
  displayed: { id: number }[];
  /** Where in `displayed` the row open in the detail pane is, or -1. */
  openIndex: number;
  searching: boolean;
}) {
  const [helpOpen, setHelpOpen] = useState(false);
  const scope = searching ? 'every row found' : 'the rows on this page';
  const bindings: Keybinding[] = [
    { keys: ['x'], description: 'Check or uncheck the open row', available: openIndex >= 0 },
    { keys: ['a', 'Ctrl+A'], description: `Check ${scope} (Cmd+A on a Mac)`, available: displayed.length > 0 },
    { keys: ['n'], description: 'Uncheck every row', available: true },
    { keys: ['i'], description: `Invert which of ${scope} are checked`, available: displayed.length > 0 },
  ];

  // Re-bound on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return;
      if (target instanceof Element && target.closest('.cm-editor, [contenteditable="true"]')) return;
      // Ctrl+A only stands for `a`, which it would otherwise select the page's text with
      const selectAllKey = e.key === 'a' && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey;
      if (!selectAllKey && (e.ctrlKey || e.metaKey || e.altKey)) return;
      if (e.key === '?') {
        e.preventDefault();
        setHelpOpen(open => !open);
        return;
      }
      // The help covers the list, so nothing should change behind it
      if (helpOpen) return;
      const actions: Record<string, () => void> = {
        x: () => rows.toggleRow(displayed, openIndex, false),
        a: () => rows.selectAll(displayed),
        n: rows.selectNone,
        i: () => rows.invert(displayed),
      };
      const binding = bindings.find(b => b.keys.includes(e.key));
      if (!binding?.available || !actions[e.key]) return;
      e.preventDefault();
      actions[e.key]();
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });

  return { open: helpOpen, onOpenChange: setHelpOpen, bindings };
}
