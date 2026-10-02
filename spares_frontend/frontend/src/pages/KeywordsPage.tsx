import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { getDuplicateKeywords, getNote, getNoteLinks, getUnmatchedKeywords, listKeywords, searchKeyword } from '../api/client';
import Navbar from '../components/Navbar';
import NoteDetail from '../components/NoteDetail';
import { useAuth } from '../hooks/useAuth';
import type { MatchedKeywordResponse, NoteLink, NoteResponse, UnmatchedKeywordResponse } from '../types/spares';
import { td, th } from '../utils';

type Tab = 'all' | 'search' | 'unmatched' | 'duplicate' | 'links';

const TABS: { tab: Tab; label: string }[] = [
  { tab: 'all', label: 'All' },
  { tab: 'search', label: 'Search' },
  { tab: 'unmatched', label: 'Unmatched' },
  { tab: 'duplicate', label: 'Duplicate' },
  { tab: 'links', label: 'Links' },
];

const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };
const hint: React.CSSProperties = { fontSize: 13, color: 'var(--text-muted)', marginTop: 0 };

/** A note id that opens the note in the side pane. */
function NoteLinkButton({ id, onOpen }: { id: number; onOpen: (id: number) => void }) {
  return (
    <a href="#" onClick={e => { e.preventDefault(); onOpen(id); }}>{id}</a>
  );
}

function formatScore(score: number | null): string {
  return score === null ? '—' : score.toFixed(3);
}

/** `spares keyword list` (`--short` dedupes the keywords). */
function AllKeywords({ onOpen }: { onOpen: (id: number) => void }) {
  const [keywords, setKeywords] = useState<[number, string][] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [short, setShort] = useState(false);
  const [filter, setFilter] = useState('');

  useEffect(() => {
    listKeywords().then(setKeywords, e => setError(String(e)));
  }, []);

  if (error) return <div style={{ color: 'var(--error)' }}>Error: {error}</div>;
  if (!keywords) return <div>Loading…</div>;

  const needle = filter.trim().toLowerCase();
  const shown = keywords.filter(([, k]) => !needle || k.toLowerCase().includes(needle));
  const unique = [...new Set(shown.map(([, k]) => k))];

  return (
    <>
      <div style={{ display: 'flex', gap: 12, alignItems: 'center', marginBottom: 12, flexWrap: 'wrap' }}>
        <input value={filter} onChange={e => setFilter(e.target.value)} placeholder="Filter…" style={{ ...input, flex: 1, maxWidth: 300 }} />
        <label style={{ fontSize: 14 }}>
          <input type="checkbox" checked={short} onChange={e => setShort(e.target.checked)} style={{ marginRight: 6 }} />
          Unique keywords only
        </label>
        <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>{short ? unique.length : shown.length} shown</span>
      </div>
      {short ? (
        <ul style={{ margin: 0, paddingLeft: 20, fontSize: 14, lineHeight: 1.7 }}>
          {unique.map(k => <li key={k}>{k}</li>)}
        </ul>
      ) : (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
          <thead><tr><th style={th}>Note</th><th style={th}>Keyword</th></tr></thead>
          <tbody>
            {shown.map(([id, k], i) => (
              <tr key={`${id}-${k}-${i}`}>
                <td style={td}><NoteLinkButton id={id} onOpen={onOpen} /></td>
                <td style={td}>{k}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

/** `spares keyword search` (the best match) and `spares keyword ranking` (every match). */
function KeywordSearch({ onOpen }: { onOpen: (id: number) => void }) {
  const [keyword, setKeyword] = useState('');
  const [results, setResults] = useState<{ keyword: string; matches: MatchedKeywordResponse[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function search() {
    const k = keyword.trim();
    if (!k) return;
    setError(null);
    try {
      setResults({ keyword: k, matches: await searchKeyword(k) });
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
        <input
          value={keyword}
          onChange={e => setKeyword(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') search(); }}
          placeholder="Keyword"
          style={{ ...input, flex: 1, maxWidth: 300 }}
        />
        <button onClick={search}>Search</button>
      </div>
      {error && <div style={{ color: 'var(--error)' }}>Error: {error}</div>}
      {results && (results.matches.length === 0 ? (
        <p style={{ color: 'var(--text-secondary)' }}>No matching keyword found for “{results.keyword}”.</p>
      ) : (
        <>
          <p style={hint}>Ranked best first; the first row is what a link to “{results.keyword}” resolves to.</p>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
            <thead><tr><th style={th}>#</th><th style={th}>Matched keyword</th><th style={th}>Note</th><th style={th}>Score</th></tr></thead>
            <tbody>
              {results.matches.map((m, i) => (
                <tr key={`${m.note_id}-${m.matched_keyword}`} style={i === 0 ? { background: 'var(--selected)' } : undefined}>
                  <td style={td}>{i + 1}</td>
                  <td style={td}>{m.matched_keyword}</td>
                  <td style={td}><NoteLinkButton id={m.note_id} onOpen={onOpen} /></td>
                  <td style={{ ...td, fontVariantNumeric: 'tabular-nums' }}>{formatScore(m.score)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ))}
    </>
  );
}

/** `spares keyword unmatched`. */
function UnmatchedKeywords({ onOpen }: { onOpen: (id: number) => void }) {
  const [rows, setRows] = useState<UnmatchedKeywordResponse[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getUnmatchedKeywords().then(setRows, e => setError(String(e)));
  }, []);

  if (error) return <div style={{ color: 'var(--error)' }}>Error: {error}</div>;
  if (!rows) return <div>Loading…</div>;
  return (
    <>
      <p style={hint}>Keywords that notes link to, but that no note has.</p>
      <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
        <thead><tr><th style={th}>Note</th><th style={th}>Searched keyword</th></tr></thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={`${r.note_id}-${r.searched_keyword}-${i}`}>
              <td style={td}><NoteLinkButton id={r.note_id} onOpen={onOpen} /></td>
              <td style={td}>{r.searched_keyword}</td>
            </tr>
          ))}
          {rows.length === 0 && <tr><td colSpan={2} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>No unmatched keywords</td></tr>}
        </tbody>
      </table>
    </>
  );
}

/** `spares keyword duplicate`. */
function DuplicateKeywords({ onOpen }: { onOpen: (id: number) => void }) {
  const [rows, setRows] = useState<[string, number[]][] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getDuplicateKeywords().then(setRows, e => setError(String(e)));
  }, []);

  if (error) return <div style={{ color: 'var(--error)' }}>Error: {error}</div>;
  if (!rows) return <div>Loading…</div>;
  return (
    <>
      <p style={hint}>Keywords on more than one note.</p>
      <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
        <thead><tr><th style={th}>Keyword</th><th style={th}>Notes</th></tr></thead>
        <tbody>
          {rows.map(([k, ids]) => (
            <tr key={k}>
              <td style={td}>{k}</td>
              <td style={td}>
                <span style={{ display: 'inline-flex', gap: 8, flexWrap: 'wrap' }}>
                  {ids.map(id => <NoteLinkButton key={id} id={id} onOpen={onOpen} />)}
                </span>
              </td>
            </tr>
          ))}
          {rows.length === 0 && <tr><td colSpan={2} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>No duplicate keywords</td></tr>}
        </tbody>
      </table>
    </>
  );
}

/** `spares link list --score-threshold`. */
function NoteLinks({ onOpen }: { onOpen: (id: number) => void }) {
  const [threshold, setThreshold] = useState('');
  const [links, setLinks] = useState<{ threshold: number; links: NoteLink[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    const t = Number(threshold);
    if (!threshold.trim() || Number.isNaN(t)) { setError(`Invalid score threshold: ${threshold}`); return; }
    setError(null);
    try {
      setLinks({ threshold: t, links: await getNoteLinks(t) });
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <p style={hint}>Matched note links scoring at or below the threshold, worst first, to find links that resolved to the wrong note.</p>
      <div style={{ display: 'flex', gap: 8, marginBottom: 12, alignItems: 'center' }}>
        <label style={{ fontSize: 14 }}>
          Score threshold{' '}
          <input
            type="number"
            step="any"
            value={threshold}
            onChange={e => setThreshold(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') load(); }}
            style={{ ...input, width: 100 }}
          />
        </label>
        <button onClick={load}>List</button>
      </div>
      {error && <div style={{ color: 'var(--error)' }}>Error: {error}</div>}
      {links && (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
          <thead>
            <tr>
              <th style={th}>Note</th>
              <th style={th}>Searched keyword</th>
              <th style={th}>Matched keyword</th>
              <th style={th}>Linked note</th>
              <th style={th}>Score</th>
            </tr>
          </thead>
          <tbody>
            {links.links.map(l => (
              <tr key={`${l.parent_note_id}-${l.order}`}>
                <td style={td}><NoteLinkButton id={l.parent_note_id} onOpen={onOpen} /></td>
                <td style={td}>{l.searched_keyword}</td>
                <td style={td}>{l.matched_keyword ?? '—'}</td>
                <td style={td}>{l.linked_note_id === null ? '—' : <NoteLinkButton id={l.linked_note_id} onOpen={onOpen} />}</td>
                <td style={{ ...td, fontVariantNumeric: 'tabular-nums' }}>{formatScore(l.score)}</td>
              </tr>
            ))}
            {links.links.length === 0 && (
              <tr><td colSpan={5} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>No links scoring at or below {links.threshold}</td></tr>
            )}
          </tbody>
        </table>
      )}
    </>
  );
}

export default function KeywordsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [tab, setTab] = useState<Tab>('all');
  const [panelNote, setPanelNote] = useState<NoteResponse | null>(null);
  const [noteError, setNoteError] = useState<string | null>(null);

  useEffect(() => {
    if (!credentials) navigate('/login');
  }, [credentials, navigate]);

  async function openNote(id: number) {
    setNoteError(null);
    try {
      setPanelNote(await getNote(id));
    } catch (e) {
      setNoteError(String(e));
    }
  }

  return (
    <div style={{ padding: 24 }}>
      <style>{`
        .keywords-split { display: flex; flex-direction: row; gap: 24px; align-items: flex-start; }
        @media (max-width: 768px) { .keywords-split { flex-direction: column; } }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Keywords</h2>

      <div className="keywords-split">
        <div style={{ flex: 1, minWidth: 0 }}>
          <div role="tablist" style={{ display: 'flex', gap: 4, borderBottom: '1px solid var(--border)', marginBottom: 16 }}>
            {TABS.map(t => (
              <button
                key={t.tab}
                role="tab"
                aria-selected={tab === t.tab}
                onClick={() => setTab(t.tab)}
                style={{
                  padding: '6px 14px',
                  fontSize: 14,
                  border: 'none',
                  borderBottom: tab === t.tab ? '2px solid var(--accent)' : '2px solid transparent',
                  background: 'none',
                  cursor: 'pointer',
                  fontWeight: tab === t.tab ? 600 : 400,
                }}
              >
                {t.label}
              </button>
            ))}
          </div>
          {tab === 'all' && <AllKeywords onOpen={openNote} />}
          {tab === 'search' && <KeywordSearch onOpen={openNote} />}
          {tab === 'unmatched' && <UnmatchedKeywords onOpen={openNote} />}
          {tab === 'duplicate' && <DuplicateKeywords onOpen={openNote} />}
          {tab === 'links' && <NoteLinks onOpen={openNote} />}
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          {noteError && <div style={{ color: 'var(--error)', fontSize: 13, marginBottom: 8 }}>{noteError}</div>}
          {panelNote
            ? <NoteDetail
                key={panelNote.id}
                note={panelNote}
                onClose={() => setPanelNote(null)}
                onNoteUpdated={setPanelNote}
                onOpenNote={openNote}
              />
            : <div style={{ color: 'var(--text-faint)', fontSize: 14, paddingTop: 8 }}>Select a note id to see it.</div>
          }
        </div>
      </div>
    </div>
  );
}
