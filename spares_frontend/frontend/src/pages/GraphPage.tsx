import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { ApiError, getNote, getNoteGraph } from '../api/client';
import Navbar from '../components/Navbar';
import NoteDetail from '../components/NoteDetail';
import NoteGraph, { type GraphLayout } from '../components/NoteGraph';
import { STUDY_COLORS, tagColor } from '../graphLayout';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import { useAuth } from '../hooks/useAuth';
import { useDetailView } from '../hooks/useDetailView';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useSavedSearches } from '../hooks/useSavedSearches';
import type { NoteGraphResponse, NoteResponse } from '../types/spares';
import { errorMessage, formatNoteId, keepShown } from '../utils';

const MAX_NODES = 2000;
const DEPTHS = [1, 2, 3, 4, 5];
const SCORES = [0, 0.5, 1, 1.5, 2.5, 4];
const input: React.CSSProperties = { padding: '4px 8px', fontSize: 13, border: '1px solid var(--border-strong)', borderRadius: 4 };
const chip: React.CSSProperties = { padding: '4px 10px', fontSize: 13 };
const legendDot = (color: string, hollow = false): React.CSSProperties => ({
  display: 'inline-block', width: 10, height: 10, borderRadius: '50%', marginRight: 4, verticalAlign: 'middle',
  border: `1.5px solid ${color}`, background: hollow ? 'transparent' : color,
});

/** The query that shows a note with the notes it builds on. */
const noteQuery = (id: number) => `id=${formatNoteId(id)}`;

/** A URL parameter's number, or `null` if it is missing or not a number. */
function numberParam(value: string | null): number | null {
  const n = value === null ? NaN : Number(value);
  return Number.isFinite(n) ? n : null;
}

export default function GraphPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  // Kept in the URL, so a graph can be linked to and is the same after going back to it
  const searchQuery = searchParams.get('q') ?? '';
  const layout: GraphLayout = searchParams.get('layout') === 'layered' ? 'layered' : 'force';
  const colorBy = (['scope', 'study', 'tag'] as const).find(c => c === searchParams.get('color')) ?? 'scope';
  const depth = numberParam(searchParams.get('depth'));
  const maxScore = numberParam(searchParams.get('score'));
  const hideIsolated = searchParams.get('isolated') === 'hide';

  const [query, setQuery] = useState(searchQuery);
  const [searchError, setSearchError] = useState<SearchInputError | null>(null);
  const [graph, setGraph] = useState<NoteGraphResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [find, setFind] = useState('');
  const [centerOn, setCenterOn] = useState<number | null>(null);
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);
  const [paneStatus, setPaneStatus] = useState<string | null>(null);
  const { recent, save: saveRecent, remove: removeRecent } = useRecentSearches('spares_graph_recent_searches');
  const { saved: savedSearches, remove: removeSavedSearch } = useSavedSearches();

  const setParam = (key: string, value: string | null) => setSearchParams(prev => {
    const next = new URLSearchParams(prev);
    if (value === null) next.delete(key);
    else next.set(key, value);
    return next;
  });

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the input follows the URL, e.g. on back
    setQuery(searchQuery);
    setLoading(true);
    let cancelled = false;
    getNoteGraph({ query: searchQuery, max_score: maxScore, ancestor_depth: depth, max_nodes: MAX_NODES })
      .then(data => {
        if (cancelled) return;
        setGraph(data);
        setError(null);
        setSearchError(null);
        if (searchQuery.trim()) saveRecent(searchQuery);
      })
      .catch(e => {
        if (cancelled) return;
        setError(errorMessage(e));
        setSearchError(e instanceof ApiError && e.span ? { query: searchQuery, span: e.span } : null);
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, searchQuery, maxScore, depth, saveRecent]);

  const shown = useMemo(() => {
    if (!graph) return { nodes: [], edges: [] };
    if (!hideIsolated) return graph;
    const linked = new Set(graph.edges.flatMap(e => [e.from, e.to]));
    return { nodes: graph.nodes.filter(n => linked.has(n.id)), edges: graph.edges };
  }, [graph, hideIsolated]);

  const marked = useMemo(() => {
    const text = find.trim().toLowerCase();
    if (!text) return new Set<number>();
    return new Set(shown.nodes.filter(n => n.keywords.some(k => k.toLowerCase().includes(text)) || formatNoteId(n.id) === text).map(n => n.id));
  }, [find, shown]);

  function search(q = query) {
    setQuery(q);
    setParam('q', q.trim() ? q : null);
  }

  async function select(id: number | null) {
    setPaneStatus(null);
    if (id === null) { setSelectedNote(null); return; }
    try {
      setSelectedNote(await getNote(id));
    } catch (e) {
      setPaneStatus(errorMessage(e));
    }
  }

  function openNote(id: number) {
    void select(id);
    setCenterOn(id);
  }

  const detail = useDetailView(selectedNote !== null, () => setSelectedNote(null));
  const inQuery = shown.nodes.filter(n => n.in_query).length;
  const topTags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const n of shown.nodes) {
      const top = n.tags[0]?.split(':')[0];
      if (top) counts.set(top, (counts.get(top) ?? 0) + 1);
    }
    return [...counts].sort((a, b) => b[1] - a[1]).slice(0, 8).map(([tag]) => tag);
  }, [shown]);

  return (
    <div className="page-wide">
      <div className="page-navbar">
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Graph</h2>

      <div className="split">
        <div style={{ flex: 2, minWidth: 0, display: detail.showList ? undefined : 'none' }}>
          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
            <SearchInput
              value={query}
              onChange={setQuery}
              onSubmit={search}
              placeholder="Notes to show, with the notes they build on, e.g. tag=calculus"
              ariaLabel="Notes to show"
              error={searchError}
            />
            <button onClick={() => search()} className="touch-target-small">Show</button>
            {searchQuery && <button onClick={() => search('')} className="touch-target-small" title="Every note">Clear</button>}
          </div>
          <RecentSearches
            heading="Saved"
            recent={savedSearches}
            label={s => s.name}
            itemTitle={s => s.query}
            onSelect={s => search(s.query)}
            onRemove={s => { if (window.confirm(`Delete the saved search "${s.name}"?`)) void removeSavedSearch(s); }}
          />
          <RecentSearches recent={recent} onSelect={search} onRemove={removeRecent} style={{ marginBottom: 8 }} />

          <div style={{ display: 'flex', gap: 12, alignItems: 'center', flexWrap: 'wrap', marginBottom: 8, fontSize: 13 }}>
            <div role="group" aria-label="Layout" style={{ display: 'flex', gap: 4 }}>
              <button style={chip} aria-pressed={layout === 'force'} onClick={() => setParam('layout', null)} title="Linked notes pull together, as in Obsidian">Force</button>
              <button style={chip} aria-pressed={layout === 'layered'} onClick={() => setParam('layout', 'layered')} title="Each note below the notes it builds on">Layered</button>
            </div>
            <label>
              Color by{' '}
              <select value={colorBy} onChange={e => setParam('color', e.target.value === 'scope' ? null : e.target.value)} style={input}>
                <option value="scope">Found or built on</option>
                <option value="study">How learned</option>
                <option value="tag">Tag</option>
              </select>
            </label>
            <label title="How many links up from the notes found to follow">
              Depth{' '}
              <select value={depth ?? ''} onChange={e => setParam('depth', e.target.value || null)} style={input}>
                <option value="">All</option>
                {DEPTHS.map(d => <option key={d} value={d}>{d}</option>)}
              </select>
            </label>
            <label title="The highest score a link can have and still be shown: 0 matches a keyword exactly, and higher scores allow plurals, typos and shared stems">
              Link score ≤{' '}
              <select value={maxScore ?? ''} onChange={e => setParam('score', e.target.value || null)} style={input}>
                <option value="">Queue's</option>
                {SCORES.map(s => <option key={s} value={s}>{s}</option>)}
              </select>
            </label>
            <label>
              <input type="checkbox" checked={hideIsolated} onChange={e => setParam('isolated', e.target.checked ? 'hide' : null)} /> Hide unlinked notes
            </label>
            <input
              type="search"
              value={find}
              onChange={e => setFind(e.target.value)}
              onKeyDown={e => {
                if (e.key !== 'Enter') return;
                const first = shown.nodes.find(n => marked.has(n.id));
                if (first) openNote(first.id);
              }}
              placeholder="Find by keyword"
              aria-label="Find a note in the graph by keyword"
              style={{ ...input, width: 160 }}
            />
          </div>

          {error && <pre style={{ color: 'var(--error)', marginBottom: 12, marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>}
          {graph && !graph.links_generated && (
            <div style={{ fontSize: 13, color: 'var(--warning)', marginBottom: 8 }}>Links between notes haven't been found yet. Render the notes, as with `spares note render`, to find them.</div>
          )}
          {graph?.truncated && (
            <div style={{ fontSize: 13, color: 'var(--warning)', marginBottom: 8 }}>Only {MAX_NODES} notes are shown. Narrow the query or the depth to see the rest.</div>
          )}
          <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8, display: 'flex', gap: 12, flexWrap: 'wrap', alignItems: 'center' }}>
            <span>
              {loading ? 'Loading…' : `${inQuery} note${inQuery === 1 ? '' : 's'} found, ${shown.nodes.length - inQuery} built on, ${shown.edges.length} link${shown.edges.length === 1 ? '' : 's'}`}
              {find.trim() && ` · ${marked.size} match${marked.size === 1 ? '' : 'es'}`}
            </span>
            <span style={{ marginLeft: 'auto' }}>
              {colorBy === 'scope' && <>
                <span style={{ marginRight: 10 }}><span style={legendDot('var(--accent)')} />Found</span>
                <span><span style={legendDot('var(--text-faint)', true)} />Built on</span>
              </>}
              {colorBy === 'study' && Object.values(STUDY_COLORS).map(({ color, label }) => (
                <span key={label} style={{ marginRight: 10 }}><span style={legendDot(color)} />{label}</span>
              ))}
              {colorBy === 'tag' && topTags.map(tag => (
                <span key={tag} style={{ marginRight: 10 }}><span style={legendDot(tagColor(tag))} />{tag}</span>
              ))}
              {colorBy !== 'scope' && <span style={{ color: 'var(--text-muted)' }}><span style={legendDot('var(--text-muted)', true)} />hollow: built on</span>}
            </span>
          </div>

          {graph && shown.nodes.length === 0 && !loading ? (
            <div style={{ color: 'var(--text-muted)', fontSize: 14, padding: '24px 0', textAlign: 'center' }}>No notes found</div>
          ) : (
            <NoteGraph
              nodes={shown.nodes}
              edges={shown.edges}
              layout={layout}
              colorBy={colorBy}
              selected={selectedNote?.id ?? null}
              onSelect={id => void select(id)}
              marked={marked}
              centerOn={centerOn}
              height="max(360px, calc(100vh - 400px))"
            />
          )}
        </div>

        {(selectedNote || paneStatus || !detail.isNarrow) && (
          <div style={{ flex: 1, minWidth: 0 }}>
            {!detail.showList && <button onClick={detail.back} className="touch-target" style={{ marginBottom: 12 }}>← Back to graph</button>}
            {paneStatus && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>{paneStatus}</div>}
            {selectedNote ? (
              <>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 8, fontSize: 13 }}>
                  <button onClick={() => { search(noteQuery(selectedNote.id)); detail.back(); }} className="touch-target-small" title="Show only this note and the notes it builds on">
                    Show what it builds on
                  </button>
                  <Link to={`/notes?q=${encodeURIComponent(noteQuery(selectedNote.id))}`}>Open in Notes</Link>
                </div>
                <NoteDetail
                  key={selectedNote.id}
                  note={selectedNote}
                  onClose={detail.back}
                  onNoteUpdated={updated => setSelectedNote(prev => keepShown(prev, updated))}
                  onOpenNote={openNote}
                />
              </>
            ) : !detail.isNarrow && (
              <div style={{ color: 'var(--text-faint)', fontSize: 14, paddingTop: 8 }}>
                Click a note to see it. Notes the query found are filled; the notes they build on are hollow.
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
