import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { getNoteGraph } from '../api/client';
import { setLocalGraphOpen, useLocalGraphOpen } from '../preferences';
import type { NoteGraphResponse } from '../types/spares';
import { errorMessage, formatNoteId } from '../utils';
import NoteGraph from './NoteGraph';

const DEPTHS = [1, 2, 3];
/** Enough for a note's neighborhood, which a few hubs could otherwise fill with every note. */
const MAX_NODES = 300;
const NO_MARKS = new Set<number>();
const NO_GROUPS: string[] = [];

/**
 * A note's local graph, as in Obsidian: the notes it links to and those that link to it, a number
 * of links away. Collapsed until opened, which is remembered.
 */
export default function LocalGraph({ noteId, onOpenNote }: { noteId: number; onOpenNote: (noteId: number) => void }) {
  const open = useLocalGraphOpen();
  const [depth, setDepth] = useState(1);
  const [graph, setGraph] = useState<NoteGraphResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const query = `id=${formatNoteId(noteId)}`;

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    getNoteGraph({ query, max_score: null, ancestor_depth: depth, descendant_depth: depth, groups: NO_GROUPS, max_nodes: MAX_NODES })
      .then(data => { if (!cancelled) { setGraph(data); setError(null); } })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); });
    return () => { cancelled = true; };
  }, [open, query, depth]);

  return (
    <details open={open} onToggle={e => setLocalGraphOpen(e.currentTarget.open)} style={{ marginBottom: 16 }}>
      <summary style={{ fontSize: 12, color: 'var(--text-muted)', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', cursor: 'pointer' }}>
        Local Graph
      </summary>
      {open && (
        <div style={{ marginTop: 6 }}>
          <div style={{ display: 'flex', gap: 12, alignItems: 'center', fontSize: 13, marginBottom: 6 }}>
            <label title="How many links to follow, both to the notes this note builds on and to those that build on it">
              Depth{' '}
              <select value={depth} onChange={e => setDepth(Number(e.target.value))} style={{ fontSize: 13 }}>
                {DEPTHS.map(d => <option key={d} value={d}>{d}</option>)}
              </select>
            </label>
            <Link to={`/graph?q=${encodeURIComponent(query)}&depth=${depth}&down=${depth}`} style={{ marginLeft: 'auto' }}>Open in Graph</Link>
          </div>
          {error && <div style={{ fontSize: 13, color: 'var(--error)' }}>{error}</div>}
          {graph && graph.nodes.length <= 1 && !error ? (
            <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
              {graph.links_generated ? 'No notes link to this note, and it links to none.' : "Links between notes haven't been found yet."}
            </div>
          ) : graph && (
            <NoteGraph
              nodes={graph.nodes}
              edges={graph.edges}
              layout="force"
              colorBy="scope"
              groupColors={NO_GROUPS}
              selected={noteId}
              onSelect={id => { if (id !== null && id !== noteId) onOpenNote(id); }}
              marked={NO_MARKS}
              centerOn={null}
              height={260}
            />
          )}
        </div>
      )}
    </details>
  );
}
