/** Keyboard shortcuts that work beyond one page, for the help popup. Each list is kept by the code
    that handles the keys, which points here. */

export interface Shortcut {
  keys: string[];
  description: string;
}

/** A page's own shortcut, which only some states of the page have. */
export interface Keybinding extends Shortcut {
  /** Whether pressing it would do something right now. */
  available: boolean;
}

/** Opens the help popup, from the navbar (`Navbar`). */
export const HELP_SHORTCUT: Shortcut = { keys: ['?'], description: 'Show or hide this help' };

/** In a search box, as `SearchInput`'s `handleKeyDown` handles them. */
export const SEARCH_BOX_SHORTCUTS: Shortcut[] = [
  { keys: ['Tab'], description: 'Accept the highlighted completion' },
  { keys: ['↑', '↓'], description: 'Move between completions' },
  { keys: ['Enter'], description: 'Search' },
  { keys: ['Esc'], description: 'Hide the completions' },
];

/** In a note's editor, as `NoteDetail` handles them. */
export const NOTE_EDITOR_SHORTCUTS: Shortcut[] = [
  { keys: [':w'], description: 'Save the note, with Vim keybindings on (in Settings)' },
];

/** In the answer boxes of a typed answer, as `TypedAnswerInputs` handles them. */
export const TYPED_ANSWER_SHORTCUTS: Shortcut[] = [
  { keys: ['Enter'], description: 'Move to the next answer box, or show the answer from the last' },
  { keys: ['Ctrl+Enter'], description: 'Show the answer from any answer box (Cmd+Enter on a Mac)' },
];
