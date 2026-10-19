/** Whether `target` takes the keys pressed in it, e.g. to type them, or to seek with the arrows.
    Checkboxes take no keys but Space, so a page's keys still work after one is clicked. */
export function takesKeys(target: EventTarget | null): boolean {
  if (target instanceof HTMLInputElement) return target.type !== 'checkbox' && target.type !== 'radio';
  if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement || target instanceof HTMLMediaElement) return true;
  return target instanceof Element && target.closest('.cm-editor, [contenteditable="true"]') !== null;
}

/** Whether a popup other than the page's help covers the page, which its keys shouldn't act behind. */
export function dialogOpen(): boolean {
  return document.querySelector('dialog[open]') !== null;
}

/**
 * How a shortcut names the key pressed: `KeyboardEvent.key`, after `Ctrl+` for Ctrl or Cmd, and
 * `Shift+` for Shift with a named key, as in `Shift+ArrowDown`. A typed character already says
 * Shift, as in `S`. Null with Alt, which no shortcut uses.
 */
export function keyName(e: Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>): string | null {
  if (e.altKey) return null;
  const named = e.key.length > 1;
  let name = e.key === ' ' ? 'Space' : e.key;
  if (named && e.shiftKey) name = `Shift+${name}`;
  if (e.ctrlKey || e.metaKey) name = `Ctrl+${name}`;
  return name;
}
