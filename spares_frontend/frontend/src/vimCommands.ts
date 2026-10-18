import { useEffect, type RefObject } from 'react';
import { Vim, type CodeMirrorV } from '@replit/codemirror-vim';

/** Ex commands for the note panes (`NoteDetail` and `NewNoteForm`), typed in any of a pane's
    editors with Vim keybindings on. Listed in the help popup as `NOTE_EDITOR_SHORTCUTS`. */
export type VimPaneCommand = 'write' | 'quit' | 'writeQuit' | 'exit';

export interface VimPaneCommandEvent {
  command: VimPaneCommand;
  /** Whether the command ends with `!`, as in `:q!`. */
  force: boolean;
  /** Shows a message in the Vim panel of the editor the command was typed in. */
  warn: (message: string) => void;
}

/** Vim's message for quitting with unsaved changes. */
export const NO_WRITE_WARNING = 'E37: No write since last change (add ! to override)';

// Ex commands are global, so the editor reaches its pane through a DOM event
const VIM_COMMAND_EVENT = 'spares-vim-command';

function showWarning(cm: CodeMirrorV, message: string) {
  // Styled as the Vim panel's own messages
  const element = document.createElement('div');
  element.className = 'cm-vim-message';
  element.style.color = 'red';
  element.textContent = message;
  cm.openNotification(element, { bottom: true, duration: 15000 });
}

function defineCommand(name: string, prefix: string, command: VimPaneCommand) {
  Vim.defineEx(name, prefix, (cm, params) => {
    const detail: VimPaneCommandEvent = {
      command,
      force: params.argString?.startsWith('!') ?? false,
      warn: message => showWarning(cm, message),
    };
    const event = new CustomEvent(VIM_COMMAND_EVENT, { bubbles: true, cancelable: true, detail });
    // Not handled outside of a note pane, e.g. in a typed answer
    if (cm.cm6.dom.dispatchEvent(event)) showWarning(cm, `E492: Not an editor command: ${params.input}`);
  });
}

defineCommand('write', 'w', 'write');
defineCommand('quit', 'q', 'quit');
defineCommand('wq', 'wq', 'writeQuit');
defineCommand('xit', 'x', 'exit');

/** Handles the ex commands typed in the editors within `rootRef`. */
export function useVimCommands(rootRef: RefObject<HTMLElement | null>, handle: (event: VimPaneCommandEvent) => void) {
  // Re-bound on every render so the handler sees the current contents
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const listener = (event: Event) => {
      event.preventDefault();
      handle((event as CustomEvent<VimPaneCommandEvent>).detail);
    };
    root.addEventListener(VIM_COMMAND_EVENT, listener);
    return () => root.removeEventListener(VIM_COMMAND_EVENT, listener);
  });
}
