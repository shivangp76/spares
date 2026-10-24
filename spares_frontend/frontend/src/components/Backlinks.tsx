import { useEffect, useState } from 'react';
import { getNoteGraph } from '../api/client';
import type { GraphEdge, GraphNode } from '../types/spares';
import { errorMessage, formatNoteId } from '../utils';

/** Enough for any note but a hub, for which it says there are more. */
const MAX_NODES = 200;
const NO_GROUPS: string[] = [];
const label: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', marginBottom: 2, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };

/**
 * The notes that link to a note, as in Obsidian's backlinks, from the same links as its graph.
 * Shows nothing if no note does.
 */
export default function Backlinks({ noteId, version, onOpenNote }: {
  noteId: number;
  /** Changed to refetch, e.g. after the note's keywords change what links to it. */
  version: string;
  onOpenNote: (noteId: number) => void;
}) {
  const [backlinks, setBacklinks] = useState<{ notes: { node: GraphNode; edge: GraphEdge }[]; truncated: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // One node more for the note itself
    getNoteGraph({ query: `id=${formatNoteId(noteId)}`, max_score: null, ancestor_depth: 0, descendant_depth: 1, groups: NO_GROUPS, max_nodes: MAX_NODES + 1 })
      .then(data => {
        if (cancelled) return;
        const nodes = new Map(data.nodes.map(n => [n.id, n]));
        const notes = data.edges
          .filter(e => e.to === noteId && e.from !== noteId)
          .flatMap(edge => {
            const node = nodes.get(edge.from);
            return node ? [{ node, edge }] : [];
          });
        setBacklinks({ notes, truncated: data.truncated });
        setError(null);
      })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); });
    return () => { cancelled = true; };
  }, [noteId, version]);

  if (!error && (!backlinks || backlinks.notes.length === 0)) return null;
  return (
    <div style={{ marginBottom: 16 }}>
      <div style={label}>Linked From</div>
      {error && <div style={{ fontSize: 13, color: 'var(--error)' }}>{error}</div>}
      {backlinks && (
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 4 }}>
          {backlinks.notes.map(({ node, edge }) => (
            <button
              key={node.id}
              onClick={() => onOpenNote(node.id)}
              title={`Links to this note's keyword "${edge.keyword}"`}
              className="button-link"
              style={{ padding: '4px 10px', fontSize: 12 }}
            >
              {node.keywords.length > 0 ? `${node.keywords[0]} (${formatNoteId(node.id)})` : formatNoteId(node.id)}
            </button>
          ))}
          {backlinks.truncated && <span style={{ fontSize: 12, color: 'var(--text-muted)', alignSelf: 'center' }}>and more</span>}
        </div>
      )}
    </div>
  );
}
