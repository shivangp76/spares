import { useEffect, useState } from 'react';
import type { Keybinding } from '../help/shortcuts';
import { dialogOpen, keyName, takesKeys } from '../keys';
import { useVimKeybindings } from '../preferences';

/** A page's own shortcut, and what it does. */
export interface PageShortcut extends Keybinding {
  /** The `keyName`s that press it, of which `keys` are how the help shows them. */
  codes: string[];
  /** Vim's keys for it, as `j` for ↓, which press it only with Vim keybindings on (in Settings). */
  vim?: string[];
  /** Given the `keyName` pressed, for a shortcut of several keys. */
  run: (code: string) => void;
}

/**
 * Presses a page's `shortcuts`, and opens its help with `?`, which lists them. Returns the `help`
 * the page's `Navbar` takes. The keys do nothing while typing in a box or an editor, or while a
 * popup is open.
 */
export function usePageKeys(shortcuts: PageShortcut[]) {
  const [helpOpen, setHelpOpen] = useState(false);
  const vimEnabled = useVimKeybindings();
  const active = shortcuts.map(s => (vimEnabled && s.vim ? { ...s, keys: [...s.keys, ...s.vim], codes: [...s.codes, ...s.vim] } : s));

  // Re-bound on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (takesKeys(e.target)) return;
      const name = keyName(e);
      if (name === null) return;
      // The help covers the page, as other popups do, so nothing should change behind it
      if (dialogOpen() && !helpOpen) return;
      if (name === '?') {
        e.preventDefault();
        setHelpOpen(open => !open);
        return;
      }
      if (helpOpen) return;
      // They press a focused button or link
      if ((name === 'Enter' || name === 'Space') && e.target instanceof Element && e.target.closest('a, button, summary')) return;
      const shortcut = active.find(s => s.codes.includes(name));
      if (!shortcut?.available) return;
      e.preventDefault();
      shortcut.run(name);
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });

  const bindings: Keybinding[] = active.map(({ keys, description, available }) => ({ keys, description, available }));
  return { open: helpOpen, onOpenChange: setHelpOpen, bindings };
}
