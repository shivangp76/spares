import { useEffect, useState } from 'react';
import CodeMirror from '@uiw/react-codemirror';
import { vim } from '@replit/codemirror-vim';
import { createNotes, listParsers, renderNote } from '../api/client';
import type { NoteResponse, ParserResponse } from '../types/spares';

const fieldLabel: React.CSSProperties = { fontSize: 12, color: '#888', marginBottom: 4, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const cmStyle = { border: '1px solid #eee', borderRadius: 4, fontSize: 13, background: '#fff' };
// Enough to list every parser on one page
const PARSERS_LIMIT = 1000;

function lines(value: string): string[] {
  return value.split('\n').map(s => s.trim()).filter(Boolean);
}

/** `spares note add`, followed by rendering the note as the CLI does when syncing it. */
export default function NewNoteForm({ onCreated, onCancel }: { onCreated: (note: NoteResponse) => void; onCancel: () => void }) {
  const [parsers, setParsers] = useState<ParserResponse[] | null>(null);
  const [parserId, setParserId] = useState<number | null>(null);
  const [data, setData] = useState('');
  const [tags, setTags] = useState('');
  const [keywords, setKeywords] = useState('');
  const [isSuspended, setIsSuspended] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listParsers(1, PARSERS_LIMIT)
      .then(list => { setParsers(list); setParserId(list[0]?.id ?? null); })
      .catch(e => setError(String(e)));
  }, []);

  async function create() {
    if (parserId === null) { setError('Select a parser'); return; }
    if (!data.trim()) { setError('Data is empty'); return; }
    setSaving(true);
    setError(null);
    try {
      const [note] = await createNotes(parserId, [{
        data,
        keywords: lines(keywords),
        tags: lines(tags),
        is_suspended: isSuspended,
        custom_data: {},
      }]);
      try {
        await renderNote(note.id);
      } catch (e) {
        // The note exists, so still hand it over
        window.alert(`Note ${note.id} created, but ${String(e)}`);
      }
      onCreated(note);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div style={{ border: '1px solid #ddd', borderRadius: 6, padding: 20, backgroundColor: '#fafafa' }}>
      <h3 style={{ marginTop: 0, marginBottom: 16, fontSize: 16 }}>New Note</h3>

      <div style={{ marginBottom: 16 }}>
        <div style={fieldLabel}>Parser</div>
        <select
          value={parserId ?? ''}
          onChange={e => setParserId(Number(e.target.value))}
          disabled={!parsers}
          style={{ padding: '6px 10px', fontSize: 14 }}
        >
          {!parsers && <option>Loading…</option>}
          {parsers?.map(p => <option key={p.id} value={p.id}>{p.name} ({p.id})</option>)}
        </select>
      </div>

      <div style={{ marginBottom: 16 }}>
        <div style={fieldLabel}>Data</div>
        <CodeMirror value={data} onChange={setData} extensions={[vim()]} basicSetup={{ lineNumbers: true }} minHeight="120px" style={cmStyle} />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px 24px', marginBottom: 16 }}>
        <div>
          <div style={fieldLabel}>Tags <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(one per line)</span></div>
          <CodeMirror value={tags} onChange={setTags} extensions={[vim()]} basicSetup={{ lineNumbers: false }} style={cmStyle} />
        </div>
        <div>
          <div style={fieldLabel}>Keywords <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(one per line)</span></div>
          <CodeMirror value={keywords} onChange={setKeywords} extensions={[vim()]} basicSetup={{ lineNumbers: false }} style={cmStyle} />
        </div>
      </div>

      <label style={{ display: 'block', fontSize: 13, marginBottom: 16 }}>
        <input type="checkbox" checked={isSuspended} onChange={e => setIsSuspended(e.target.checked)} style={{ marginRight: 6 }} />
        Suspend its cards
      </label>

      <div style={{ display: 'flex', gap: 10, alignItems: 'center' }}>
        <button onClick={create} disabled={saving || parserId === null} style={{ padding: '5px 14px', fontSize: 13 }}>
          {saving ? 'Creating…' : 'Create'}
        </button>
        <button onClick={onCancel} disabled={saving} style={{ padding: '5px 14px', fontSize: 13 }}>Cancel</button>
        {error && <span style={{ fontSize: 13, color: 'red' }}>{error}</span>}
      </div>
    </div>
  );
}
