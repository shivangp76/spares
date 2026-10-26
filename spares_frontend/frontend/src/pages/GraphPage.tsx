import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { ApiError, getNote, getNoteGraph } from '../api/client';
import { NavbarHelp } from '../components/Navbar';
import NoteDetail from '../components/NoteDetail';
import NoteGraph, { type GraphLayout, type NoteGraphHandle } from '../components/NoteGraph';
import { DEFAULT_DISPLAY, DEFAULT_FORCES, GROUP_COLORS, RELATION_COLORS, STUDY_COLORS, formatGroup, parseGroup, tagColor, unmatchedNodes, type GraphDisplay, type GraphForces, type GraphGroup } from '../graphLayout';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import { useAuth } from '../hooks/useAuth';
import { useDetailView } from '../hooks/useDetailView';
import { usePageKeys } from '../hooks/usePageKeys';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useSavedSearches } from '../hooks/useSavedSearches';
import type { GraphNode, GraphRelation, NoteGraphResponse, NoteResponse } from '../types/spares';
import { errorMessage, formatNoteId, keepShown } from '../utils';

const MAX_NODES = 2000;
/** How far the arrow keys move the graph, and with Shift. */
const PAN_STEP = 60;
const FAST_PAN_STEP = 240;
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

/** How many of `nodes` are there for each reason. */
function countRelations(nodes: GraphNode[]): Record<GraphRelation, number> {
  const counts = { found: 0, prerequisite: 0, dependent: 0, unmatched: 0 };
  for (const n of nodes) counts[n.relation]++;
  return counts;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** How this browser draws the graph, kept apart from the URL, which says what it shows. */
const SETTINGS_KEY = 'spares_graph_settings';

interface GraphSettings { display: GraphDisplay; forces: GraphForces }

function loadSettings(): GraphSettings {
  try {
    const stored = JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') as Partial<GraphSettings>;
    return { display: { ...DEFAULT_DISPLAY, ...stored.display }, forces: { ...DEFAULT_FORCES, ...stored.forces } };
  } catch {
    // Without storage, or with something else stored, the defaults are used
    return { display: DEFAULT_DISPLAY, forces: DEFAULT_FORCES };
  }
}

function saveSettings(settings: GraphSettings): void {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // Without storage the settings last until the page is left
  }
}

const GUIDE_KEY = 'spares_graph_guide_open';
const guideTerm: React.CSSProperties = { fontWeight: 600 };
const guideList: React.CSSProperties = { margin: '4px 0 8px', paddingLeft: 20, lineHeight: 1.5 };

/**
 * What the graph shows and what each of its settings does, open until it is first closed. The
 * help popup says less, to point here.
 */
function GraphGuide() {
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(GUIDE_KEY) !== 'closed';
    } catch {
      return true;
    }
  });
  function toggle(nowOpen: boolean) {
    setOpen(nowOpen);
    try {
      localStorage.setItem(GUIDE_KEY, nowOpen ? 'open' : 'closed');
    } catch {
      // Without storage it opens again next time
    }
  }
  return (
    <details open={open} onToggle={e => toggle(e.currentTarget.open)} style={{ marginBottom: 8, fontSize: 13, color: 'var(--text-secondary)' }}>
      <summary style={{ cursor: 'pointer' }}>What do these mean?</summary>
      <div style={{ border: '1px solid var(--border-subtle)', borderRadius: 4, padding: '8px 12px', marginTop: 6, background: 'var(--surface)' }}>
        <div style={guideTerm}>Reading the graph</div>
        <ul style={guideList}>
          <li>A note <em>builds on</em> another when it links to one of that note's keywords. An arrow goes from a note to each note that builds on it, so arrows point from what to learn first to what comes after.</li>
          <li>Filled notes are the ones the query found. Hollow notes are shown because of how they link to those: gray ones are <em>built on</em> by a note found, and purple ones are <em>building on</em> a note found. Dashed ones are keywords notes link to that no note has yet.</li>
          <li>Point at a note to light up everything it builds on and everything building on it. Click it to open it beside the graph.</li>
          <li>Drag the background, or use the arrow keys (faster with Shift), to move around. Scroll, or press <kbd>+</kbd> and <kbd>-</kbd>, to zoom. In the Force layout, drag a note to move it.</li>
        </ul>
        <div style={guideTerm}>Settings</div>
        <ul style={guideList}>
          <li><strong>Force / Layered:</strong> Force pulls linked notes together and pushes the rest apart, as in Obsidian. Layered puts every note below all the notes it builds on, so it reads top to bottom in the order to learn them.</li>
          <li><strong>Built on:</strong> how many links to follow up from the notes found, to the notes they build on. 1 shows only the notes they link to directly; All follows the whole chain, even to notes the query doesn't find.</li>
          <li><strong>Building on them:</strong> how many links to follow down from the notes found, to the notes that link to them. None, the default, leaves those out unless the query finds them too. Turn it up to see what a note holds back.</li>
          <li>
            <strong>Link score ≤:</strong> links are found by matching a link's text to keywords loosely, and each link's score says how loosely: 0 when they are the same apart from case, 0.5 for punctuation, accents or a possessive, 1 for a plural, a stop word or words run together, 1.5 and 2.5 for one or two typos in a long word, and 2.5 and up for words that only share a stem.
            Links scoring higher than this aren't shown or followed. <em>Queue's</em> uses <code>linked_note_max_score</code> from the server's <code>config.toml</code> (1.5 unless set), the same cutoff review uses to hold cards back until the notes they build on are learned.
          </li>
          <li><strong>Color by:</strong> <em>Found or built on</em> colors notes by why they are shown. <em>How learned</em> colors them by their cards: not studied (all new), partly learned, learned (all in review), suspended, or no cards. <em>Tag</em> colors them by their first tag's top level, so <code>math:algebra</code> and <code>math:calculus</code> match.</li>
          <li><strong>Hide unlinked notes:</strong> leaves out notes with no links to or from any note shown.</li>
          <li><strong>Unmatched links:</strong> shows the keywords notes link to that no note has. Writing a note with that keyword gives the links somewhere to go.</li>
          <li><strong>Find by keyword:</strong> rings the notes with a keyword containing the text, without changing what is shown. Enter opens the first.</li>
          <li><strong>Groups:</strong> each colors the notes its query finds, over the other colors. A note takes the first group that finds it.</li>
          <li><strong>Display and forces:</strong> how notes are drawn, and how strongly they pull and push in the Force layout. These are kept in this browser.</li>
        </ul>
        <div>At most {MAX_NODES} notes are shown at once: the notes found first, then the nearest ones linked to them.</div>
      </div>
    </details>
  );
}

/** A slider with its label and value. */
function Slider({ label, title, value, min, max, step, onChange, disabled }: {
  label: string; title: string; value: number; min: number; max: number; step: number; onChange: (value: number) => void; disabled?: boolean;
}) {
  return (
    <label title={title} style={{ display: 'flex', alignItems: 'center', gap: 6, opacity: disabled ? 0.5 : 1 }}>
      <span style={{ minWidth: 92 }}>{label}</span>
      <input type="range" min={min} max={max} step={step} value={value} disabled={disabled} onChange={e => onChange(Number(e.target.value))} style={{ width: 120 }} />
      <span style={{ minWidth: 32, color: 'var(--text-muted)' }}>{value}</span>
    </label>
  );
}

export default function GraphPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  // Kept in the URL, so a graph can be linked to and is the same after going back to it
  const searchQuery = searchParams.get('q') ?? '';
  const layout: GraphLayout = searchParams.get('layout') === 'layered' ? 'layered' : 'force';
  const colorBy = (['scope', 'study', 'tag'] as const).find(c => c === searchParams.get('color')) ?? 'scope';
  const depth = numberParam(searchParams.get('depth'));
  // None unless asked for, and every one for `all`
  const downParam = searchParams.get('down');
  const descendantDepth = downParam === 'all' ? null : numberParam(downParam) ?? 0;
  const maxScore = numberParam(searchParams.get('score'));
  const hideIsolated = searchParams.get('isolated') === 'hide';
  const showUnmatched = searchParams.get('unmatched') === 'show';
  const groupParams = searchParams.getAll('g');
  // A string, to refetch only when the groups change, not each time the URL is read
  const groupsKey = JSON.stringify(groupParams);
  const groups = useMemo(() => (JSON.parse(groupsKey) as string[]).map(parseGroup), [groupsKey]);
  // Edited here until a query is submitted, when they go in the URL
  const [groupDrafts, setGroupDrafts] = useState<GraphGroup[]>(groups);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the boxes follow the URL, e.g. on back
    setGroupDrafts(groups);
  }, [groups]);

  const [query, setQuery] = useState(searchQuery);
  const [searchError, setSearchError] = useState<SearchInputError | null>(null);
  const [graph, setGraph] = useState<NoteGraphResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [find, setFind] = useState('');
  const [settings, setSettings] = useState(loadSettings);
  const changeSettings = (change: (s: GraphSettings) => GraphSettings) => setSettings(s => {
    const next = change(s);
    saveSettings(next);
    return next;
  });
  const setDisplay = (change: Partial<GraphDisplay>) => changeSettings(s => ({ ...s, display: { ...s.display, ...change } }));
  const setForces = (change: Partial<GraphForces>) => changeSettings(s => ({ ...s, forces: { ...s.forces, ...change } }));
  const [centerOn, setCenterOn] = useState<number | null>(null);
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);
  // A keyword no note has, picked in the graph
  const [selectedMissing, setSelectedMissing] = useState<GraphNode | null>(null);
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
    const groupQueries = groups.map(g => g.query);
    getNoteGraph({ query: searchQuery, max_score: maxScore, ancestor_depth: depth, descendant_depth: descendantDepth, groups: groupQueries, max_nodes: MAX_NODES })
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
  }, [credentials, navigate, searchQuery, maxScore, depth, descendantDepth, groups, saveRecent]);

  const shown = useMemo(() => {
    if (!graph) return { nodes: [], edges: [] };
    let { nodes, edges } = graph;
    if (showUnmatched) {
      const missing = unmatchedNodes(graph.unmatched);
      nodes = [...nodes, ...missing.nodes];
      edges = [...edges, ...missing.edges];
    }
    if (hideIsolated) {
      const linked = new Set(edges.flatMap(e => [e.from, e.to]));
      nodes = nodes.filter(n => linked.has(n.id));
    }
    return { nodes, edges };
  }, [graph, hideIsolated, showUnmatched]);

  function saveGroups(next: GraphGroup[]) {
    setGroupDrafts(next);
    setSearchParams(prev => {
      const params = new URLSearchParams(prev);
      params.delete('g');
      for (const group of next) params.append('g', formatGroup(group));
      return params;
    });
  }

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
    setSelectedMissing(null);
    if (id === null) { setSelectedNote(null); return; }
    if (id < 0) {
      setSelectedNote(null);
      setSelectedMissing(shown.nodes.find(n => n.id === id) ?? null);
      return;
    }
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

  const detail = useDetailView(selectedNote !== null || selectedMissing !== null, () => { setSelectedNote(null); setSelectedMissing(null); });
  const graphView = useRef<NoteGraphHandle>(null);
  const showingGraph = detail.showList && shown.nodes.length > 0;
  const keys = usePageKeys([
    // `=` is `+` without Shift
    { keys: ['+'], codes: ['+', '='], description: 'Zoom in on the graph', available: showingGraph, run: () => graphView.current?.zoomIn() },
    { keys: ['-'], codes: ['-'], description: 'Zoom out of the graph', available: showingGraph, run: () => graphView.current?.zoomOut() },
    {
      keys: ['←', '↑', '→', '↓'],
      codes: ['ArrowLeft', 'ArrowUp', 'ArrowRight', 'ArrowDown', 'Shift+ArrowLeft', 'Shift+ArrowUp', 'Shift+ArrowRight', 'Shift+ArrowDown'],
      description: 'Move around the graph, faster with Shift',
      available: showingGraph,
      run: code => {
        const step = code.startsWith('Shift+') ? FAST_PAN_STEP : PAN_STEP;
        const [dx, dy] = { ArrowLeft: [-1, 0], ArrowUp: [0, -1], ArrowRight: [1, 0], ArrowDown: [0, 1] }[code.replace('Shift+', '')] ?? [0, 0];
        graphView.current?.pan(dx * step, dy * step);
      },
    },
  ]);
  const counts = countRelations(shown.nodes);
  const missingFrom = selectedMissing
    ? [...new Set((graph?.unmatched ?? []).filter(u => u.keyword.toLowerCase() === selectedMissing.keywords[0].toLowerCase()).map(u => u.from))]
    : [];
  const groupColors = groups.map(g => GROUP_COLORS[g.color]);
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
      <NavbarHelp help={keys} />
      <h2 style={{ marginBottom: 16 }}>Graph</h2>

      <div className="split">
        <div className="split-list" style={{ flex: 2, minWidth: 0, display: detail.showList ? undefined : 'none' }}>
          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
            <SearchInput
              value={query}
              onChange={setQuery}
              onSubmit={search}
              placeholder="Notes to show, with the notes they build on, e.g. tag=calculus"
              ariaLabel="Notes to show"
              error={searchError}
              style={{ maxWidth: 'none' }}
              inputStyle={{ fontSize: 15, padding: '8px 12px' }}
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
            <label title="How many links up from the notes found to follow, to the notes they build on">
              Built on{' '}
              <select value={depth ?? ''} onChange={e => setParam('depth', e.target.value || null)} style={input}>
                <option value="">All</option>
                {DEPTHS.map(d => <option key={d} value={d}>{d}</option>)}
              </select>
            </label>
            <label title="How many links down from the notes found to follow, to the notes that build on them, even those the query doesn't find">
              Building on them{' '}
              <select value={downParam === 'all' ? 'all' : descendantDepth ?? 0} onChange={e => setParam('down', e.target.value === '0' ? null : e.target.value)} style={input}>
                <option value="0">None</option>
                {DEPTHS.map(d => <option key={d} value={d}>{d}</option>)}
                <option value="all">All</option>
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
            <label title="Show the keywords notes link to that no note has, which are notes yet to be written">
              <input type="checkbox" checked={showUnmatched} onChange={e => setParam('unmatched', e.target.checked ? 'show' : null)} /> Unmatched links
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

          <GraphGuide />

          <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 8, fontSize: 13 }}>
            <span title="Color the notes a query finds, over the colors above. A note takes the first group that finds it." style={{ color: 'var(--text-secondary)' }}>Groups</span>
            {groupDrafts.map((group, i) => (
              <span key={i} style={{ display: 'inline-flex', gap: 4, alignItems: 'center' }}>
                <select
                  value={group.color}
                  onChange={e => saveGroups(groupDrafts.map((g, j) => (j === i ? { ...g, color: e.target.value } : g)))}
                  aria-label={`Color of group ${i + 1}`}
                  style={{ ...input, color: GROUP_COLORS[group.color], fontWeight: 600 }}
                >
                  {Object.keys(GROUP_COLORS).map(c => <option key={c} value={c}>● {c}</option>)}
                </select>
                <SearchInput
                  value={group.query}
                  onChange={q => setGroupDrafts(groupDrafts.map((g, j) => (j === i ? { ...g, query: q } : g)))}
                  onSubmit={q => saveGroups(groupDrafts.map((g, j) => (j === i ? { ...g, query: q } : g)))}
                  placeholder="e.g. tag=exam"
                  ariaLabel={`Query of group ${i + 1}, applied with Enter`}
                  style={{ minWidth: 140, maxWidth: 220 }}
                  inputStyle={input}
                />
                <button onClick={() => saveGroups(groupDrafts.filter((_, j) => j !== i))} style={{ padding: '2px 8px', fontSize: 12 }} aria-label={`Remove group ${i + 1}`}>×</button>
              </span>
            ))}
            <button
              onClick={() => {
                const used = new Set(groupDrafts.map(g => g.color));
                const color = Object.keys(GROUP_COLORS).find(c => !used.has(c)) ?? 'blue';
                setGroupDrafts([...groupDrafts, { color, query: '' }]);
              }}
              style={chip}
            >
              New group
            </button>
          </div>

          <details style={{ marginBottom: 8, fontSize: 13 }}>
            <summary style={{ cursor: 'pointer', color: 'var(--text-secondary)' }}>Display and forces</summary>
            <div style={{ display: 'flex', gap: 24, flexWrap: 'wrap', marginTop: 8 }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <label>
                  Size by{' '}
                  <select value={settings.display.sizeBy} onChange={e => setDisplay({ sizeBy: e.target.value as GraphDisplay['sizeBy'] })} style={input}>
                    <option value="cards">Cards</option>
                    <option value="links">Links</option>
                  </select>
                </label>
                <Slider label="Note size" title="How big notes are drawn" value={settings.display.nodeSize} min={0.5} max={2.5} step={0.1} onChange={nodeSize => setDisplay({ nodeSize })} />
                <Slider label="Link thickness" title="How thick links are drawn" value={settings.display.linkWidth} min={0.5} max={4} step={0.25} onChange={linkWidth => setDisplay({ linkWidth })} />
                <Slider label="Names from zoom" title="The zoom from which every note is named. Names fade in on the way to it. Notes in focus are always named." value={settings.display.labelZoom} min={0.1} max={3} step={0.1} onChange={labelZoom => setDisplay({ labelZoom })} />
                <label>
                  <input type="checkbox" checked={settings.display.arrows} onChange={e => setDisplay({ arrows: e.target.checked })} /> Arrows
                </label>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <Slider label="Center force" title="How strongly notes are pulled to the middle: higher makes the graph rounder and tighter" value={settings.forces.center} min={0} max={0.3} step={0.01} onChange={center => setForces({ center })} disabled={layout !== 'force'} />
                <Slider label="Repel force" title="How strongly notes push each other apart" value={settings.forces.repel} min={0} max={600} step={10} onChange={repel => setForces({ repel })} disabled={layout !== 'force'} />
                <Slider label="Link force" title="How strongly links pull their notes together" value={settings.forces.link} min={0} max={2} step={0.1} onChange={link => setForces({ link })} disabled={layout !== 'force'} />
                <Slider label="Link distance" title="How long links try to be" value={settings.forces.distance} min={10} max={250} step={5} onChange={distance => setForces({ distance })} disabled={layout !== 'force'} />
                {layout !== 'force' && <span style={{ color: 'var(--text-muted)' }}>Forces apply to the Force layout.</span>}
              </div>
            </div>
            <button onClick={() => changeSettings(() => ({ display: DEFAULT_DISPLAY, forces: DEFAULT_FORCES }))} style={{ ...chip, marginTop: 8 }}>Restore defaults</button>
          </details>

          {error && <pre style={{ color: 'var(--error)', marginBottom: 12, marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>}
          {graph && !graph.links_generated && (
            <div style={{ fontSize: 13, color: 'var(--warning)', marginBottom: 8 }}>Links between notes haven't been found yet. Render the notes, as with `spares note render`, to find them.</div>
          )}
          {graph?.truncated && (
            <div style={{ fontSize: 13, color: 'var(--warning)', marginBottom: 8 }}>Only {MAX_NODES} notes are shown. Narrow the query or the depth to see the rest.</div>
          )}
          <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8, display: 'flex', gap: 12, flexWrap: 'wrap', alignItems: 'center' }}>
            <span>
              {loading ? 'Loading…' : [
                `${plural(counts.found, 'note')} found`,
                `${counts.prerequisite} built on`,
                counts.dependent > 0 ? `${counts.dependent} building on them` : null,
                counts.unmatched > 0 ? `${plural(counts.unmatched, 'keyword')} with no note` : null,
                plural(shown.edges.length, 'link'),
              ].filter(Boolean).join(', ')}
              {find.trim() && ` · ${marked.size} match${marked.size === 1 ? '' : 'es'}`}
            </span>
            <span style={{ marginLeft: 'auto' }}>
              {groups.map((g, i) => g.query.trim() && (
                <span key={`group-${i}`} style={{ marginRight: 10 }}><span style={legendDot(GROUP_COLORS[g.color])} />{g.query}</span>
              ))}
              {colorBy === 'scope' && (['found', 'prerequisite', 'dependent'] as const).filter(r => r === 'found' || counts[r] > 0).map(r => (
                <span key={r} style={{ marginRight: 10 }}><span style={legendDot(RELATION_COLORS[r].color, r !== 'found')} />{RELATION_COLORS[r].label}</span>
              ))}
              {colorBy === 'study' && Object.values(STUDY_COLORS).map(({ color, label }) => (
                <span key={label} style={{ marginRight: 10 }}><span style={legendDot(color)} />{label}</span>
              ))}
              {colorBy === 'tag' && topTags.map(tag => (
                <span key={tag} style={{ marginRight: 10 }}><span style={legendDot(tagColor(tag))} />{tag}</span>
              ))}
              {colorBy !== 'scope' && <span style={{ color: 'var(--text-muted)', marginRight: 10 }}><span style={legendDot('var(--text-muted)', true)} />hollow: not found</span>}
              {counts.unmatched > 0 && <span style={{ color: 'var(--text-muted)' }}><span style={{ ...legendDot(RELATION_COLORS.unmatched.color, true), borderStyle: 'dashed' }} />{RELATION_COLORS.unmatched.label}</span>}
            </span>
          </div>

          {graph && shown.nodes.length === 0 && !loading ? (
            <div style={{ color: 'var(--text-muted)', fontSize: 14, padding: '24px 0', textAlign: 'center' }}>No notes found</div>
          ) : (
            <NoteGraph
              ref={graphView}
              nodes={shown.nodes}
              edges={shown.edges}
              layout={layout}
              colorBy={colorBy}
              groupColors={groupColors}
              selected={selectedNote?.id ?? selectedMissing?.id ?? null}
              onSelect={id => void select(id)}
              marked={marked}
              centerOn={centerOn}
              height="max(360px, calc(100vh - 400px))"
              display={settings.display}
              forces={settings.forces}
            />
          )}
        </div>

        {(selectedNote || selectedMissing || paneStatus || !detail.isNarrow) && (
          <div style={{ flex: 1, minWidth: 0 }}>
            {!detail.showList && <button onClick={detail.back} className="touch-target" style={{ marginBottom: 12 }}>← Back to graph</button>}
            {paneStatus && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>{paneStatus}</div>}
            {selectedMissing ? (
              <div style={{ fontSize: 14 }}>
                <p style={{ marginTop: 0 }}>No note has the keyword <strong>{selectedMissing.keywords[0]}</strong> yet. {plural(missingFrom.length, 'note links', 'notes link')} to it:</p>
                <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                  {missingFrom.map(id => (
                    <button key={id} onClick={() => openNote(id)} className="button-link" style={{ padding: '4px 10px', fontSize: 12 }}>
                      {shown.nodes.find(n => n.id === id)?.keywords[0] ?? formatNoteId(id)} ({formatNoteId(id)})
                    </button>
                  ))}
                </div>
                <p style={{ color: 'var(--text-muted)', fontSize: 13 }}>Write a note with this keyword, on the Notes page, for the links to it to find it.</p>
              </div>
            ) : selectedNote ? (
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
                  localGraph={false}
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
