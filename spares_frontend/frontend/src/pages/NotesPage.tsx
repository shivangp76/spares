import { useEffect, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { listNotes, searchNotes, updateNote } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import Navbar from '../components/Navbar';
import type { NoteResponse } from '../types/spares';
import CodeMirror from '@uiw/react-codemirror';
import { vim } from '@replit/codemirror-vim';

const PAGE_SIZE = 20;
const DATA_PREVIEW_LEN = 100;
const th: React.CSSProperties = { textAlign: 'left', padding: '8px 12px', borderBottom: '1px solid #ccc' };
const td: React.CSSProperties = { padding: '8px 12px', borderBottom: '1px solid #eee' };
const dataTd: React.CSSProperties = { ...td, maxWidth: 300, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };

const fieldLabel: React.CSSProperties = { fontSize: 12, color: '#888', marginBottom: 4, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const metaLabel: React.CSSProperties = { fontSize: 12, color: '#888', marginBottom: 2, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const cmStyle = { border: '1px solid #eee', borderRadius: 4, fontSize: 13 };

function NoteDetail({ note, onClose, onNoteUpdated }: { note: NoteResponse; onClose: () => void; onNoteUpdated: (updated: NoteResponse) => void }) {
  const [dataContent, setDataContent] = useState(note.data);
  const [tagsContent, setTagsContent] = useState(note.tags.join('\n'));
  const [keywordsContent, setKeywordsContent] = useState(note.keywords.join('\n'));
  const [saveStatus, setSaveStatus] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    setDataContent(note.data);
    setTagsContent(note.tags.join('\n'));
    setKeywordsContent(note.keywords.join('\n'));
    setSaveStatus('idle');
    setSaveError(null);
  }, [note.id]);

  async function handleSave() {
    setSaveStatus('saving');
    setSaveError(null);
    const tags = tagsContent.split('\n').map(s => s.trim()).filter(Boolean);
    const keywords = keywordsContent.split('\n').map(s => s.trim()).filter(Boolean);
    try {
      const updated = await updateNote(note.id, dataContent, tags, keywords);
      onNoteUpdated(updated);
      setSaveStatus('saved');
      setTimeout(() => setSaveStatus('idle'), 2000);
    } catch (e) {
      setSaveError(String(e));
      setSaveStatus('error');
    }
  }

  return (
    <div style={{ border: '1px solid #ddd', borderRadius: 6, padding: 20, position: 'relative', backgroundColor: '#fafafa' }}>
      <button
        onClick={onClose}
        style={{ position: 'absolute', top: 12, right: 12, background: 'none', border: 'none', fontSize: 18, cursor: 'pointer', color: '#666', lineHeight: 1 }}
        aria-label="Close detail"
      >×</button>
      <h3 style={{ marginTop: 0, marginBottom: 16, fontSize: 16 }}>Note #{note.id}</h3>

      <div style={{ marginBottom: 16 }}>
        <div style={fieldLabel}>Data</div>
        <CodeMirror
          value={dataContent}
          onChange={setDataContent}
          extensions={[vim()]}
          basicSetup={{ lineNumbers: true }}
          style={cmStyle}
        />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px 24px', marginBottom: 16 }}>
        <div>
          <div style={fieldLabel}>Tags <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(one per line)</span></div>
          <CodeMirror
            value={tagsContent}
            onChange={setTagsContent}
            extensions={[vim()]}
            basicSetup={{ lineNumbers: false }}
            style={cmStyle}
          />
        </div>
        <div>
          <div style={fieldLabel}>Keywords <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(one per line)</span></div>
          <CodeMirror
            value={keywordsContent}
            onChange={setKeywordsContent}
            extensions={[vim()]}
            basicSetup={{ lineNumbers: false }}
            style={cmStyle}
          />
        </div>
        <div>
          <div style={metaLabel}>Cards</div>
          <div style={{ fontSize: 13 }}>{note.card_count}</div>
        </div>
        <div>
          <div style={metaLabel}>Parser ID</div>
          <div style={{ fontSize: 13 }}>{note.parser_id}</div>
        </div>
        <div>
          <div style={metaLabel}>Created At</div>
          <div style={{ fontSize: 13 }}>{new Date(note.created_at).toLocaleString()}</div>
        </div>
        <div>
          <div style={metaLabel}>Updated At</div>
          <div style={{ fontSize: 13 }}>{new Date(note.updated_at).toLocaleString()}</div>
        </div>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 16 }}>
        <button
          onClick={handleSave}
          disabled={saveStatus === 'saving'}
          style={{ padding: '5px 14px', fontSize: 13, cursor: saveStatus === 'saving' ? 'not-allowed' : 'pointer' }}
        >
          {saveStatus === 'saving' ? 'Saving…' : 'Save'}
        </button>
        {saveStatus === 'saved' && <span style={{ fontSize: 13, color: '#2a7' }}>Saved</span>}
        {saveStatus === 'error' && <span style={{ fontSize: 13, color: 'red' }}>{saveError}</span>}
      </div>
    </div>
  );
}

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

      <Navbar onLogout={logout} />
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
