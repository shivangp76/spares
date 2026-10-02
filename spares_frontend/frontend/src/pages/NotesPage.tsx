import { useEffect, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { cachedNotesPage, deleteNotes, getNote, listNotes, searchNotes } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import { useRecentSearches } from '../hooks/useRecentSearches';
import Navbar from '../components/Navbar';
import RecentSearches from '../components/RecentSearches';
import NewNoteForm from '../components/NewNoteForm';
import NoteDetail from '../components/NoteDetail';
import type { NoteResponse } from '../types/spares';
import { td, th, withSearch } from '../utils';

const PAGE_SIZE = 20;
const DATA_PREVIEW_LEN = 100;
const dataTd: React.CSSProperties = { ...td, maxWidth: 300, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };

export default function NotesPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.max(1, parseInt(searchParams.get('page') ?? '1', 10));
  const [notes, setNotes] = useState<NoteResponse[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // The search is kept in the URL so it survives navigating away and back
  const searchQuery = searchParams.get('q');
  const [query, setQuery] = useState(searchQuery ?? '');
  const [searchResults, setSearchResults] = useState<NoteResponse[] | null>(null);
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);
  const [creating, setCreating] = useState(false);
  const [paneStatus, setPaneStatus] = useState<string | null>(null);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (searchQuery !== null) return;
    // Show the page from an earlier visit straight away while it is refetched
    const cached = cachedNotesPage(page, PAGE_SIZE);
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    if (cached) setNotes(cached);
    setLoading(cached === undefined);
    let cancelled = false;
    listNotes(page, PAGE_SIZE)
      .then(data => {
        if (cancelled) return;
        setNotes(data);
        setError(null);
        // Prefetch the next page so Next shows it straight away
        if (data.length === PAGE_SIZE) listNotes(page + 1, PAGE_SIZE).catch(() => {});
      })
      .catch(e => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, searchQuery]);

  const { recent: recentSearches, save: saveRecentSearch, remove: removeRecentSearch } = useRecentSearches('spares_notes_recent_searches');
  // Bumped by each search so searching the same query again refetches it
  const [searchCount, setSearchCount] = useState(0);

  useEffect(() => {
    if (!credentials) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the input follows the URL, e.g. on back
    setQuery(searchQuery ?? '');
    if (searchQuery === null) { setSearchResults(null); return; }
    setLoading(true);
    setError(null);
    let cancelled = false;
    searchNotes(searchQuery)
      .then(data => { if (!cancelled) { setSearchResults(data); saveRecentSearch(searchQuery); setError(null); } })
      .catch(e => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, searchQuery, searchCount, saveRecentSearch]);

  function handleSearch(q = query) {
    if (!q.trim()) return;
    setQuery(q);
    setSearchParams(prev => withSearch(prev, q));
    setSearchCount(n => n + 1);
  }

  function handleClear() {
    setSearchParams(prev => withSearch(prev, null));
  }

  const displayedNotes = searchResults ?? notes;
  // -1 if the selected note was opened from a link and isn't in the table
  const selectedIndex = selectedNote ? displayedNotes.findIndex(n => n.id === selectedNote.id) : -1;

  function selectNote(note: NoteResponse | null) {
    setSelectedNote(note);
    setCreating(false);
    setPaneStatus(null);
  }

  async function openLinkedNote(noteId: number) {
    try {
      selectNote(await getNote(noteId));
    } catch (e) {
      setPaneStatus(String(e));
    }
  }

  async function deleteSelected() {
    if (!selectedNote) return;
    const id = selectedNote.id;
    if (!window.confirm(`Delete note ${id} and its cards?`)) return;
    try {
      await deleteNotes({ Ids: [id] });
      setNotes(prev => prev.filter(n => n.id !== id));
      setSearchResults(prev => (prev ? prev.filter(n => n.id !== id) : prev));
      setSelectedNote(null);
      setPaneStatus(`Note ${id} deleted.`);
    } catch (e) {
      setPaneStatus(String(e));
    }
  }

  return (
    <div style={{ padding: 24 }}>
      <style>{`
        .notes-split { display: flex; flex-direction: row; gap: 24px; align-items: flex-start; }
        @media (max-width: 768px) { .notes-split { flex-direction: column; } }
        .notes-row:hover { background-color: var(--hover); }
        .notes-row-selected { background-color: var(--selected) !important; }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Notes</h2>

      <div className="notes-split">
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', gap: 8, marginBottom: recentSearches.length > 0 ? 8 : 16 }}>
            <input
              type="text"
              value={query}
              onChange={e => setQuery(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleSearch()}
              placeholder="Search notes…"
              style={{ padding: '6px 10px', fontSize: 14, flex: 1, maxWidth: 400 }}
            />
            <button onClick={() => handleSearch()}>Search</button>
            {searchResults !== null && <button onClick={handleClear}>Clear</button>}
            <button onClick={() => { setSelectedNote(null); setPaneStatus(null); setCreating(true); }} style={{ marginLeft: 'auto' }}>New note</button>
          </div>
          <RecentSearches recent={recentSearches} onSelect={handleSearch} onRemove={removeRecentSearch} style={{ marginBottom: 16 }} />

          {searchResults !== null && (
            <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>
              {searchResults.length} result{searchResults.length !== 1 ? 's' : ''} for "{searchQuery}"
            </div>
          )}

          {error && <div style={{ color: 'var(--error)', marginBottom: 12 }}>Error: {error}</div>}
          {loading && <div>Loading…</div>}

          {!loading && (
            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
              <thead>
                <tr>
                  <th style={th}>ID</th>
                  <th style={th}>Data</th>
                  <th style={th}>Tags</th>
                  <th style={th}>Keywords</th>
                  <th style={th}>Cards</th>
                </tr>
              </thead>
              <tbody>
                {displayedNotes.map(note => (
                  <tr
                    key={note.id}
                    className={`notes-row${selectedNote?.id === note.id ? ' notes-row-selected' : ''}`}
                    onClick={() => selectNote(selectedNote?.id === note.id ? null : note)}
                    style={{ cursor: 'pointer' }}
                  >
                    <td style={td}>{note.id}</td>
                    <td style={dataTd} title={note.data}>
                      {note.data.length > DATA_PREVIEW_LEN
                        ? note.data.slice(0, DATA_PREVIEW_LEN) + '…'
                        : note.data}
                    </td>
                    <td style={td}>{note.tags.join(', ') || '—'}</td>
                    <td style={td}>
                      {note.keywords.slice(0, 4).join(', ')}
                      {note.keywords.length > 4 ? '…' : ''}
                    </td>
                    <td style={td}>{note.card_count}</td>
                  </tr>
                ))}
                {displayedNotes.length === 0 && (
                  <tr><td colSpan={5} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>No notes found</td></tr>
                )}
              </tbody>
            </table>
          )}

          {searchResults === null && (
            <div style={{ marginTop: 16, display: 'flex', gap: 8, alignItems: 'center' }}>
              <button disabled={page <= 1} onClick={() => setSearchParams({ page: String(page - 1) })}>Prev</button>
              <span style={{ fontSize: 13 }}>Page {page}</span>
              <button disabled={notes.length < PAGE_SIZE} onClick={() => setSearchParams({ page: String(page + 1) })}>Next</button>
            </div>
          )}
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          {paneStatus && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>{paneStatus}</div>}
          {creating ? (
            <NewNoteForm
              onCreated={note => {
                selectNote(note);
                setPaneStatus(`Note ${note.id} created.`);
              }}
              onCancel={() => setCreating(false)}
            />
          ) : selectedNote ? (
            <>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 8, fontSize: 13 }}>
                {selectedIndex >= 0 && (
                  <>
                    {/* Wraps around at either end, as in the CLI */}
                    <button onClick={() => selectNote(displayedNotes[(selectedIndex + displayedNotes.length - 1) % displayedNotes.length])} disabled={displayedNotes.length < 2}>Previous</button>
                    <span>Note {selectedIndex + 1} of {displayedNotes.length}</span>
                    <button onClick={() => selectNote(displayedNotes[(selectedIndex + 1) % displayedNotes.length])} disabled={displayedNotes.length < 2}>Next</button>
                  </>
                )}
                <button onClick={deleteSelected} style={{ marginLeft: 'auto', color: 'var(--danger)' }}>Delete note</button>
              </div>
              <NoteDetail
                key={selectedNote.id}
                note={selectedNote}
                onClose={() => selectNote(null)}
                onNoteUpdated={(updated) => {
                  setSelectedNote(updated);
                  setNotes(prev => prev.map(n => n.id === updated.id ? updated : n));
                  setSearchResults(prev => prev ? prev.map(n => n.id === updated.id ? updated : n) : prev);
                }}
                onOpenNote={openLinkedNote}
              />
            </>
          ) : (
            <div style={{ color: 'var(--text-faint)', fontSize: 14, paddingTop: 8 }}>Select a note to see details.</div>
          )}
        </div>
      </div>
    </div>
  );
}
