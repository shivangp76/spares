import { askText } from "./ask";
import { formatNoteId } from "./utils";

/**
 * The query for the notes created in the `days` days up to note `noteId`, as the manual's "Getting
 * notes created `n` days before a note" writes it: the notes a forgotten note may build on, such as
 * those of the lectures before it.
 */
export function notesBeforeQuery(noteId: number, days: number): string {
  const id = formatNoteId(noteId);
  return `created_at<=note[${id}].created_at and created_at>=note[${id}].created_at-${days}d`;
}

/** Asks how many days before note `noteId` to look, and gives the query, or null if cancelled. */
export async function askNotesBefore(noteId: number): Promise<string | null> {
  const days = await askText({
    title: `Notes created before note ${formatNoteId(noteId)}`,
    message:
      "The notes created in the days up to this one, e.g. to review what it builds on. Add a tag to the search afterwards to narrow it.",
    confirmLabel: "Search",
    input: { label: "Days before", initial: "5" },
  });
  if (days === null) return null;
  const n = Number(days);
  return Number.isInteger(n) && n > 0 ? notesBeforeQuery(noteId, n) : null;
}
