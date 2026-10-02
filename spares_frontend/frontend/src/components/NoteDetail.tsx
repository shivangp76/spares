import { useState } from 'react';
import CodeMirror from '@uiw/react-codemirror';
import { vim } from '@replit/codemirror-vim';
import { updateNote } from '../api/client';
import type { NoteResponse } from '../types/spares';

const fieldLabel: React.CSSProperties = { fontSize: 12, color: '#888', marginBottom: 4, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const metaLabel: React.CSSProperties = { fontSize: 12, color: '#888', marginBottom: 2, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const cmStyle = { border: '1px solid #eee', borderRadius: 4, fontSize: 13 };

export default function NoteDetail({ note, onClose, onNoteUpdated }: { note: NoteResponse; onClose: () => void; onNoteUpdated: (updated: NoteResponse) => void }) {
  const [dataContent, setDataContent] = useState(note.data);
  const [tagsContent, setTagsContent] = useState(note.tags.join('\n'));
  const [keywordsContent, setKeywordsContent] = useState(note.keywords.join('\n'));
  const [saveStatus, setSaveStatus] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [saveError, setSaveError] = useState<string | null>(null);

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
