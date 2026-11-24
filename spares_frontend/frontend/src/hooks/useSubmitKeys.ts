import { useEffect, useEffectEvent } from "react";
import { keymap, Prec } from "@uiw/react-codemirror";

const SUBMIT_EVENT = "spares-submit";

/**
 * Ctrl+Enter (Cmd+Enter on a Mac) in a note editor saves the note, without Vim's `:w`, and
 * Ctrl+Shift+Enter saves it and starts another. The editor sends the pane around it an event,
 * which `useSubmitKeys` turns into a call, so this extension never changes.
 */
export const submitKeymap = Prec.highest(
  keymap.of([
    { key: "Mod-Enter", run: (view) => sendSubmit(view.dom, false) },
    { key: "Mod-Shift-Enter", run: (view) => sendSubmit(view.dom, true) },
  ]),
);

/** Whether the pane took the key, by cancelling the event. */
function sendSubmit(editor: HTMLElement, more: boolean): boolean {
  const event = new CustomEvent(SUBMIT_EVENT, {
    bubbles: true,
    cancelable: true,
    detail: more,
  });
  return !editor.dispatchEvent(event);
}

/**
 * Calls `submit` when an editor inside `rootRef` with `submitKeymap` is sent Ctrl+Enter, with
 * `more` for Ctrl+Shift+Enter. Returning false leaves the key to the editor, as a pane without
 * Create and New does for Ctrl+Shift+Enter.
 */
export function useSubmitKeys(
  rootRef: React.RefObject<HTMLElement | null>,
  submit: (more: boolean) => boolean,
) {
  const onSubmit = useEffectEvent((e: Event) => {
    if (submit((e as CustomEvent<boolean>).detail)) e.preventDefault();
  });
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    root.addEventListener(SUBMIT_EVENT, onSubmit);
    return () => root.removeEventListener(SUBMIT_EVENT, onSubmit);
  }, [rootRef]);
}
