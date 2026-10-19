import { NAV_LINKS } from '../navLinks';

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

/** Go to another page, from the navbar (`Navbar`). */
export const GO_TO_SHORTCUTS: Shortcut[] = NAV_LINKS.map(({ label, key }) => ({ keys: [`g ${key}`], description: `Go to ${label}` }));

/** In a search box, as `SearchInput`'s `handleKeyDown` handles them. */
export const SEARCH_BOX_SHORTCUTS: Shortcut[] = [
  { keys: ['Tab'], description: 'Accept the highlighted completion' },
  { keys: ['↑', '↓'], description: 'Move between completions' },
  { keys: ['Enter'], description: 'Search' },
  { keys: ['Esc'], description: 'Hide the completions' },
];

/** In the editors of a note or a new note, with Vim keybindings on, as `NoteDetail` and
    `NewNoteForm` handle the commands of `vimCommands`. */
export const NOTE_EDITOR_SHORTCUTS: Shortcut[] = [
  { keys: [':w'], description: 'Save the note (or create the new note), with Vim keybindings on (in Settings)' },
  { keys: [':q'], description: 'Close the note, unless it has unsaved changes' },
  { keys: [':q!'], description: 'Close the note, dropping its unsaved changes' },
  { keys: [':wq', ':x'], description: 'Save and close the note' },
  { keys: [':e!'], description: 'Undo the changes made since the note was last saved (or clear the new note)' },
];

/** In the answer boxes of a typed answer, as `TypedAnswerInputs` handles them. */
export const TYPED_ANSWER_SHORTCUTS: Shortcut[] = [
  { keys: ['Enter'], description: 'Move to the next answer box, or show the answer from the last' },
  { keys: ['Ctrl+Enter'], description: 'Show the answer from any answer box (Cmd+Enter on a Mac)' },
];
