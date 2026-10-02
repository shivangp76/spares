import { useEffect, useRef, useState } from 'react';
import CodeMirror from '@uiw/react-codemirror';
import { Vim, vim } from '@replit/codemirror-vim';
import { getNoteRender, renderNote, updateNote } from '../api/client';
import { useResolvedTheme } from '../theme';
import type { NoteRenderResponse, NoteResponse } from '../types/spares';
import CardRenderer from './CardRenderer';

const fieldLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', marginBottom: 4, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const metaLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', marginBottom: 2, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const cmStyle = { border: '1px solid var(--border-subtle)', borderRadius: 4, fontSize: 13 };

// `:w` saves the note being edited. Ex commands are global, so the editor reaches its note's
// NoteDetail through a DOM event.
const VIM_WRITE_EVENT = 'spares-vim-write';
Vim.defineEx('write', 'w', cm => {
  cm.cm6.dom.dispatchEvent(new Event(VIM_WRITE_EVENT, { bubbles: true }));
});

interface Props {
  note: NoteResponse;
  onClose: () => void;
  onNoteUpdated: (updated: NoteResponse) => void;
  /** Opens a linked note. The note's links are listed iff this is given. */
  onOpenNote?: (noteId: number) => void;
}

export default function NoteDetail({ note, onClose, onNoteUpdated, onOpenNote }: Props) {
  const theme = useResolvedTheme();
  const [dataContent, setDataContent] = useState(note.data);
  const [tagsContent, setTagsContent] = useState(note.tags.join('\n'));
  const [keywordsContent, setKeywordsContent] = useState(note.keywords.join('\n'));
  const [saveStatus, setSaveStatus] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [saveError, setSaveError] = useState<string | null>(null);
  // Bumped after each save so the compiled note is refetched
  const [renderVersion, setRenderVersion] = useState(0);
  const [render, setRender] = useState<{ version: number; result: NoteRenderResponse } | { version: number; error: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    getNoteRender(note.id).then(
      result => { if (!cancelled) setRender({ version: renderVersion, result }); },
      (e: unknown) => { if (!cancelled) setRender({ version: renderVersion, error: String(e) }); },
    );
    return () => { cancelled = true; };
  }, [note.id, renderVersion]);

  // Re-bound on every render so the handler sees the current contents
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const onWrite = () => { if (saveStatus !== 'saving') handleSave(); };
    root.addEventListener(VIM_WRITE_EVENT, onWrite);
    return () => root.removeEventListener(VIM_WRITE_EVENT, onWrite);
  });

  async function handleSave() {
    setSaveStatus('saving');
    setSaveError(null);
    const tags = tagsContent.split('\n').map(s => s.trim()).filter(Boolean);
    const keywords = keywordsContent.split('\n').map(s => s.trim()).filter(Boolean);
    try {
      const updated = await updateNote(note.id, dataContent, tags, keywords);
      // Render before notifying so callers see the regenerated files
      let renderError: string | null = null;
      try {
        await renderNote(note.id);
      } catch (e) {
        renderError = String(e);
      }
      onNoteUpdated(updated);
      setRenderVersion(v => v + 1);
      if (renderError) {
        setSaveError(`Saved, but ${renderError}`);
        setSaveStatus('error');
        return;
      }
      setSaveStatus('saved');
      setTimeout(() => setSaveStatus('idle'), 2000);
    } catch (e) {
      setSaveError(String(e));
      setSaveStatus('error');
    }
  }

  return (
    <div ref={rootRef} style={{ border: '1px solid var(--border)', borderRadius: 6, padding: 20, position: 'relative', backgroundColor: 'var(--surface)' }}>
      <button
        onClick={onClose}
        style={{ position: 'absolute', top: 12, right: 12, background: 'none', border: 'none', fontSize: 18, cursor: 'pointer', color: 'var(--text-secondary)', lineHeight: 1 }}
        aria-label="Close detail"
      >×</button>
      <h3 style={{ marginTop: 0, marginBottom: 16, fontSize: 16 }}>Note #{note.id}</h3>

      <div style={{ marginBottom: 16 }}>
        <div style={fieldLabel}>Data</div>
        <CodeMirror
          theme={theme}
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
            theme={theme}
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
            theme={theme}
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

      {onOpenNote && note.linked_notes && note.linked_notes.length > 0 && (
        <div style={{ marginBottom: 16 }}>
          <div style={metaLabel}>Linked Notes</div>
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 4 }}>
            {note.linked_notes.map((ln, i) => {
              const id = ln.linked_note_id;
              return (
                <button
                  key={`${i}-${ln.searched_keyword}`}
                  onClick={id === null ? undefined : () => onOpenNote(id)}
                  disabled={id === null}
                  title={id === null ? 'No note matches this keyword' : ln.matched_keyword ?? undefined}
                  style={{ padding: '4px 8px', fontSize: 12 }}
                >
                  {ln.searched_keyword} ({id ?? 'unmatched'})
                </button>
              );
            })}
          </div>
        </div>
      )}

      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 16 }}>
        <button
          onClick={handleSave}
          disabled={saveStatus === 'saving'}
          style={{ padding: '5px 14px', fontSize: 13, cursor: saveStatus === 'saving' ? 'not-allowed' : 'pointer' }}
        >
          {saveStatus === 'saving' ? 'Saving…' : 'Save'}
        </button>
        {saveStatus === 'saved' && <span style={{ fontSize: 13, color: 'var(--success)' }}>Saved</span>}
        {saveStatus === 'error' && <span style={{ fontSize: 13, color: 'var(--error)' }}>{saveError}</span>}
      </div>

      <div>
        <div style={fieldLabel}>Compiled</div>
        <div style={{ border: '1px solid var(--border-subtle)', borderRadius: 4, padding: 4, backgroundColor: 'var(--bg)' }}>
          {render === null && <div>Loading…</div>}
          {render && 'error' in render && <div style={{ color: 'var(--error)', fontSize: 13 }}>{render.error}</div>}
          {render && 'result' in render && (
            <CardRenderer
              path={render.result.rendered_path}
              parserName={render.result.parser_name}
              source={render.result.browser_source}
              version={render.version}
            />
          )}
        </div>
      </div>
    </div>
  );
}
