import { useEffect, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { listNotes, searchNotes } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import Navbar from '../components/Navbar';
import NoteDetail from '../components/NoteDetail';
import type { NoteResponse } from '../types/spares';
import { td, th } from '../utils';

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
  const [query, setQuery] = useState('');
  const [searchResults, setSearchResults] = useState<NoteResponse[] | null>(null);
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    setLoading(true);
    listNotes(page, PAGE_SIZE)
      .then(data => { setNotes(data); setError(null); })
      .catch(e => setError(String(e)))
      .finally(() => setLoading(false));
  }, [credentials, navigate, page]);

  function handleSearch() {
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    searchNotes(query)
      .then(data => { setSearchResults(data); setError(null); })
      .catch(e => setError(String(e)))
      .finally(() => setLoading(false));
  }

  function handleClear() {
    setSearchResults(null);
    setQuery('');
  }

  const displayedNotes = searchResults ?? notes;

  return (
    <div style={{ padding: 24 }}>
      <style>{`
        .notes-split { display: flex; flex-direction: row; gap: 24px; align-items: flex-start; }
        @media (max-width: 768px) { .notes-split { flex-direction: column; } }
        .notes-row:hover { background-color: #f5f5f5; }
        .notes-row-selected { background-color: #f0f4ff !important; }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Notes</h2>

      <div className="notes-split">
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
            <input
              type="text"
              value={query}
              onChange={e => setQuery(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleSearch()}
              placeholder="Search notes…"
              style={{ padding: '6px 10px', fontSize: 14, flex: 1, maxWidth: 400 }}
            />
            <button onClick={handleSearch}>Search</button>
            {searchResults !== null && <button onClick={handleClear}>Clear</button>}
          </div>

          {searchResults !== null && (
            <div style={{ fontSize: 13, color: '#555', marginBottom: 8 }}>
              {searchResults.length} result{searchResults.length !== 1 ? 's' : ''} for "{query}"
            </div>
          )}

          {error && <div style={{ color: 'red', marginBottom: 12 }}>Error: {error}</div>}
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
                    onClick={() => setSelectedNote(selectedNote?.id === note.id ? null : note)}
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
                  <tr><td colSpan={5} style={{ ...td, color: '#888', textAlign: 'center' }}>No notes found</td></tr>
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
          {selectedNote
            ? <NoteDetail
                key={selectedNote.id}
                note={selectedNote}
                onClose={() => setSelectedNote(null)}
                onNoteUpdated={(updated) => {
                  setSelectedNote(updated);
                  setNotes(prev => prev.map(n => n.id === updated.id ? updated : n));
                  setSearchResults(prev => prev ? prev.map(n => n.id === updated.id ? updated : n) : prev);
                }}
              />
            : <div style={{ color: '#999', fontSize: 14, paddingTop: 8 }}>Select a note to see details.</div>
          }
        </div>
      </div>
    </div>
  );
}
