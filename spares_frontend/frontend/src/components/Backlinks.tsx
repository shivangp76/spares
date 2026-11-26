import { useEffect, useState } from "react";
import { searchNotesPage } from "../api/client";
import type { LinkRelation } from "../types/spares";
import { errorMessage, formatNoteId } from "../utils";

/** Enough for any note but a hub, for which it says there are more. */
const MAX_NODES = 200;
const label: React.CSSProperties = {
  fontSize: 12,
  color: "var(--text-muted)",
  marginBottom: 2,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: "0.05em",
};

/**
 * The notes that link to a note, as in Obsidian's backlinks, whichever way their links point.
 * Shows nothing if no note does.
 */
interface Backlink {
  id: number;
  /** Its first keyword, to name it by */
  keyword: string | null;
  /** The keywords of this note its links matched */
  linkedBy: string[];
  relations: LinkRelation[];
}

/** What the links of a note that links to this one say, for its tooltip. */
function backlinkTitle({ linkedBy, relations }: Backlink): string {
  const keywords = [...new Set(linkedBy)].map((k) => `"${k}"`).join(", ");
  const by = keywords ? ` by ${keywords}` : "";
  if (relations.includes("prerequisite")) return `Builds on this note: links to it${by}`;
  if (relations.includes("dependent")) return `This note builds on it: links to it${by} as a note that builds on it`;
  return `Related: links to it${by}, and neither builds on the other`;
}

/**
 * A muted word after a note that doesn't build on this one: a prerequisite of this note, by its
 * dependent link, or a note that is only related.
 */
function relationMark(relations: LinkRelation[]) {
  if (relations.includes("prerequisite")) return null;
  const word = relations.includes("dependent") ? "prerequisite" : "related";
  return <span style={{ color: "var(--text-muted)", marginLeft: 4 }}>· {word}</span>;
}

export default function Backlinks({
  noteId,
  version,
  onOpenNote,
}: {
  noteId: number;
  /** Changed to refetch, e.g. after the note's keywords change what links to it. */
  version: string;
  onOpenNote: (noteId: number) => void;
}) {
  const [backlinks, setBacklinks] = useState<{
    notes: Backlink[];
    truncated: boolean;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // One more for the note itself
    searchNotesPage(`linked_to_note=${formatNoteId(noteId)}`, 1, MAX_NODES + 1)
      .then((page) => {
        if (cancelled) return;
        const notes = page.items
          .filter(({ note }) => note.id !== noteId)
          .slice(0, MAX_NODES)
          .map(({ note }): Backlink => {
            const links = (note.linked_notes ?? []).filter((l) => l.linked_note_id === noteId);
            return {
              id: note.id,
              keyword: note.keywords[0] ?? null,
              linkedBy: links.map((l) => l.matched_keyword ?? l.searched_keyword),
              relations: [...new Set(links.map((l) => l.relation ?? "prerequisite"))],
            };
          });
        setBacklinks({ notes, truncated: page.total > page.items.length });
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [noteId, version]);

  if (!error && (!backlinks || backlinks.notes.length === 0)) return null;
  return (
    <div style={{ marginBottom: 16 }}>
      <div style={label}>Linked From</div>
      {error && <div className="error-text-small">{error}</div>}
      {backlinks && (
        <div
          style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 4 }}
        >
          {backlinks.notes.map((note) => (
            <button
              key={note.id}
              onClick={() => onOpenNote(note.id)}
              title={backlinkTitle(note)}
              className="button-link button-small"
            >
              {note.keyword !== null
                ? `${note.keyword} (${formatNoteId(note.id)})`
                : formatNoteId(note.id)}
              {relationMark(note.relations)}
            </button>
          ))}
          {backlinks.truncated && (
            <span
              style={{
                fontSize: 12,
                color: "var(--text-muted)",
                alignSelf: "center",
              }}
            >
              and more
            </span>
          )}
        </div>
      )}
    </div>
  );
}
