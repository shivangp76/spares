import { useEffect, useRef, useState } from "react";
import { dataChanged } from "../dataVersion";
import { useDraftStore } from "../noteDrafts";
import { takesKeys } from "../keys";
import { captureTags } from "../quickCapture";
import type { NoteResponse } from "../types/spares";
import { formatNoteId } from "../utils";
import NewNoteForm, { type NewNoteStart } from "./NewNoteForm";
import type { NotePaneHandle } from "./NoteDetail";

/**
 * Quick capture: a new note over whatever page is open, from `c` or the command palette's New
 * note. It starts with the tags of what the page shows (`quickCapture.ts`), unless a draft of a
 * new note is waiting, which it carries on, as the Notes and Review pages do.
 */
export default function QuickCapture({
  start: given,
  onClose,
  onCreated,
}: {
  /** The note to start from, instead of the page's tags or a draft */
  start?: NewNoteStart;
  onClose: () => void;
  /** Says what was created, as the navbar's status */
  onCreated: (message: string) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const pane = useRef<NotePaneHandle>(null);
  const { newNoteDraft } = useDraftStore();
  // Read once, as the form reads its start once
  const [start] = useState<NewNoteStart | undefined>(() => {
    if (given) return given;
    const tags = captureTags();
    return newNoteDraft.load() || tags.length === 0 ? undefined : { tags };
  });

  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    requestAnimationFrame(() => pane.current?.focus());
  }, []);

  function created(note: NoteResponse, more: boolean) {
    onCreated(
      `Note ${formatNoteId(note.id)} created.${more ? " Writing another like it." : ""}`,
    );
    dataChanged();
    if (!more) dialogRef.current?.close();
  }

  return (
    <dialog
      ref={dialogRef}
      className="audio-recorder quick-capture"
      aria-label="New note"
      onClose={onClose}
      onCancel={(e) => {
        // Escape is Vim's, in the editor: the form's Cancel or :q closes it
        if (takesKeys(document.activeElement)) e.preventDefault();
      }}
    >
      <div className="audio-recorder-content">
        <NewNoteForm
          ref={pane}
          start={start}
          onCreated={created}
          onCancel={() => dialogRef.current?.close()}
        />
      </div>
    </dialog>
  );
}
