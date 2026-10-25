import { useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import CodeMirror, { EditorView, type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { getCM } from '@replit/codemirror-vim';
import { getNoteRender, listNoteImageOcclusions, renderNote, updateNote, updateNoteImageOcclusion } from '../api/client';
import { EDITOR_TIPS, useEditorSetup } from '../hooks/useEditorSetup';
import { useMediaQuery } from '../hooks/useMediaQuery';
import { useVimKeybindings } from '../preferences';
import { useResolvedTheme } from '../theme';
import type { ImageOcclusionData, NoteRenderResponse, NoteResponse } from '../types/spares';
import { SYNC_CONFLICT_TAG, errorMessage, formatNoteId, insertBlock, insertInline, insertSource, isLiveNote } from '../utils';
import { NO_WRITE_WARNING, useVimCommands } from '../vimCommands';
import Backlinks from './Backlinks';
import CardRenderer from './CardRenderer';
import ImageOcclusionEditor from './ImageOcclusionEditor';
import InsertAudioButton from './InsertAudioButton';
import InsertImageOcclusionButton from './InsertImageOcclusionButton';
import LocalGraph from './LocalGraph';
import NoteSyntaxButton from './NoteSyntaxHelp';

const fieldLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', marginBottom: 4, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const metaLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', marginBottom: 2, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const cmStyle = { border: '1px solid var(--border-subtle)', borderRadius: 4 };

const VIM_TIP = 'With Vim keybindings, :w saves the note, :q closes it (:q! drops unsaved changes), and :wq or :x saves and closes it. :e! undoes the changes since the note was last saved.';

/** What a page's keys do to a note pane (`NoteDetail` or `NewNoteForm`). */
export interface NotePaneHandle {
  /** Puts the caret in the note's text. */
  focus: () => void;
  /** Closes the pane, as its close button does, asking first if that drops unsaved changes. */
  close: () => void;
}

interface Props {
  note: NoteResponse;
  onClose: () => void;
  onNoteUpdated: (updated: NoteResponse) => void;
  /** Opens a linked note. The note's links both ways, and its local graph, are shown iff this is given. */
  onOpenNote?: (noteId: number) => void;
  /** Whether to offer the note's local graph, which a page already showing the graph leaves out. */
  localGraph?: boolean;
  ref?: React.Ref<NotePaneHandle>;
}

export default function NoteDetail({ note, onClose, onNoteUpdated, onOpenNote, localGraph = true, ref }: Props) {
  const theme = useResolvedTheme();
  // A live note's text comes from a file on the laptop, which would overwrite edits made here
  const live = isLiveNote(note);
  const editor = useEditorSetup(note.parser_name, live ? null : note.parser_id);
  const vimEnabled = useVimKeybindings();
  const [dataContent, setDataContent] = useState(note.data);
  // The editor's data as last saved, so edits made since can be told apart
  const [savedData, setSavedData] = useState(note.data);
  const dataEditorRef = useRef<ReactCodeMirrorRef>(null);
  // Set once typing into a live note's data was blocked, to say why nothing happened
  const [liveEditTried, setLiveEditTried] = useState(false);
  const dataExtensions = useMemo(
    () => live
      ? [...editor.dataExtensions, EditorView.domEventHandlers({ beforeinput: () => { setLiveEditTried(true); return false; } })]
      : editor.dataExtensions,
    [live, editor.dataExtensions],
  );
  const isNarrow = useMediaQuery('(max-width: 640px)');
  const [imageOcclusions, setImageOcclusions] = useState<ImageOcclusionData[]>([]);
  const [imageOcclusionsError, setImageOcclusionsError] = useState<string | null>(null);
  const [editingOcclusion, setEditingOcclusion] = useState<{ index: number; occlusion: ImageOcclusionData } | null>(null);
  const [tagsContent, setTagsContent] = useState(note.tags.join('\n'));
  const [savedTags, setSavedTags] = useState(tagsContent);
  const [keywordsContent, setKeywordsContent] = useState(note.keywords.join('\n'));
  const [savedKeywords, setSavedKeywords] = useState(keywordsContent);
  const unsaved = dataContent !== savedData || tagsContent !== savedTags || keywordsContent !== savedKeywords;
  const [saveStatus, setSaveStatus] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [saveError, setSaveError] = useState<string | null>(null);
  // Bumped after each save so the compiled note is refetched
  const [renderVersion, setRenderVersion] = useState(0);
  const [render, setRender] = useState<{ version: number; result: NoteRenderResponse } | { version: number; error: string } | null>(null);
  const [copyStatus, setCopyStatus] = useState<'idle' | 'copied' | 'error'>('idle');

  useEffect(() => {
    let cancelled = false;
    getNoteRender(note.id).then(
      result => { if (!cancelled) setRender({ version: renderVersion, result }); },
      (e: unknown) => { if (!cancelled) setRender({ version: renderVersion, error: errorMessage(e) }); },
    );
    return () => { cancelled = true; };
  }, [note.id, renderVersion]);

  useEffect(() => {
    let cancelled = false;
    listNoteImageOcclusions(note.id).then(
      list => { if (!cancelled) { setImageOcclusions(list); setImageOcclusionsError(null); } },
      (e: unknown) => { if (!cancelled) setImageOcclusionsError(errorMessage(e)); },
    );
    return () => { cancelled = true; };
  }, [note.id, savedData]);

  const rootRef = useRef<HTMLDivElement>(null);
  useVimCommands(rootRef, ({ command, force, warn }) => {
    if (command === 'edit') {
      if (force) {
        setDataContent(savedData);
        setTagsContent(savedTags);
        setKeywordsContent(savedKeywords);
      } else if (unsaved) {
        warn(NO_WRITE_WARNING);
      }
      return;
    }
    if (command === 'quit') {
      if (force || !unsaved) onClose();
      else warn(NO_WRITE_WARNING);
      return;
    }
    if (saveStatus === 'saving') return;
    if (command === 'write') void handleSave();
    // `:x` only saves if there is something to save, as in Vim
    else if (command === 'exit' && !unsaved) onClose();
    // Left open on an error, to show it
    else void handleSave().then(saved => { if (saved) onClose(); });
  });

  /** Renders the updated note and hands it to the caller. Returns the render error, if any. */
  async function renderAndNotify(updated: NoteResponse): Promise<string | null> {
    // Render before notifying so callers see the regenerated files
    let renderError: string | null = null;
    try {
      await renderNote(note.id);
    } catch (e) {
      renderError = errorMessage(e);
    }
    onNoteUpdated(updated);
    setRenderVersion(v => v + 1);
    return renderError;
  }

  /** The server stores the edited clozes as new files and updates the note's text to use them. */
  async function saveImageOcclusion(index: number, clozesSvg: string) {
    const updated = await updateNoteImageOcclusion(note.id, index, clozesSvg);
    setSavedData(updated.data);
    setDataContent(updated.data);
    setEditingOcclusion(null);
    const renderError = await renderAndNotify(updated);
    if (renderError) {
      setSaveError(`Saved the image occlusion, but ${renderError}`);
      setSaveStatus('error');
    }
  }

  /** Returns whether the note was saved and rendered. */
  async function handleSave(): Promise<boolean> {
    setSaveStatus('saving');
    setSaveError(null);
    const tags = tagsContent.split('\n').map(s => s.trim()).filter(Boolean);
    const keywords = keywordsContent.split('\n').map(s => s.trim()).filter(Boolean);
    try {
      const updated = await updateNote(note.id, dataContent, tags, keywords);
      // The editor keeps the text as typed, though the server may have added card orders to it
      setSavedData(dataContent);
      setSavedTags(tagsContent);
      setSavedKeywords(keywordsContent);
      const renderError = await renderAndNotify(updated);
      if (renderError) {
        setSaveError(`Saved, but ${renderError}`);
        setSaveStatus('error');
        return false;
      }
      setSaveStatus('saved');
      setTimeout(() => setSaveStatus('idle'), 2000);
      return true;
    } catch (e) {
      setSaveError(errorMessage(e));
      setSaveStatus('error');
      return false;
    }
  }

  /** Copies the data as shown, with any unsaved edits, e.g. to paste into an LLM. */
  async function copyData() {
    try {
      await navigator.clipboard.writeText(dataContent);
      setCopyStatus('copied');
    } catch {
      setCopyStatus('error');
    }
    setTimeout(() => setCopyStatus('idle'), 2000);
  }

  function close() {
    if (!unsaved || window.confirm('Discard the unsaved changes to this note?')) onClose();
  }

  useImperativeHandle(ref, () => ({ focus: () => dataEditorRef.current?.view?.focus(), close }));

  return (
    <div ref={rootRef} style={{ border: '1px solid var(--border)', borderRadius: 6, padding: 20, position: 'relative', backgroundColor: 'var(--surface)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: saveStatus === 'error' ? 8 : 16 }}>
        <h3 style={{ margin: 0, fontSize: 16, flex: 1 }}>Note {formatNoteId(note.id)}</h3>
        {saveStatus === 'saved' && <span style={{ fontSize: 13, color: 'var(--success)' }}>Saved</span>}
        <button
          onClick={handleSave}
          disabled={saveStatus === 'saving'}
          className="button-primary touch-target-small"
          style={{ padding: '5px 14px', fontSize: 13, cursor: saveStatus === 'saving' ? 'not-allowed' : 'pointer' }}
        >
          {saveStatus === 'saving' ? 'Saving…' : 'Save'}
        </button>
        <button
          onClick={close}
          style={{ fontSize: 18, lineHeight: 1, padding: '4px 8px' }}
          aria-label="Close detail"
          className="button-icon tap-area"
        >×</button>
      </div>
      {/* Below the header so a long error wraps instead of squeezing the title */}
      {saveStatus === 'error' && <div style={{ marginBottom: 16, fontSize: 13, color: 'var(--error)' }}>{saveError}</div>}
      {note.tags.includes(SYNC_CONFLICT_TAG) && (
        <div style={{ marginBottom: 12, fontSize: 13 }}>
          <span className="sync-conflict-badge">conflict</span> A sync could not merge this note. Resolve it on the laptop with <code>spares sync conflicts</code>.
        </div>
      )}
      {live && (
        <div
          role={liveEditTried ? 'alert' : undefined}
          style={{
            marginBottom: 12,
            padding: '8px 10px',
            borderRadius: 4,
            fontSize: 13,
            color: liveEditTried ? 'var(--tone-red-fg)' : 'var(--tone-orange-fg)',
            border: `1px solid ${liveEditTried ? 'var(--tone-red-border)' : 'var(--tone-orange-border)'}`,
            background: liveEditTried ? 'var(--tone-red-bg)' : 'var(--tone-orange-bg)',
          }}
        >
          {liveEditTried ? <strong>The data of a live note is read-only here. </strong> : <strong>Live note: the data is read-only here. </strong>}
          Its text comes from <code>{String(note.custom_data.live_sync_name)}</code> on the laptop, which would overwrite edits made here, so edit that file instead. Tags and keywords can be edited here.
        </div>
      )}

      <div style={{ marginBottom: 16 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 }}>
          <div style={fieldLabel}>Data{live && <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}> (read-only)</span>}</div>
          {/* Wraps on phones rather than pushing the page sideways */}
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <button
              onClick={copyData}
              title="Copy the note's data to the clipboard"
              className="button-tool"
              style={{ padding: '2px 8px', fontSize: 12, color: copyStatus === 'error' ? 'var(--error)' : undefined }}
            >
              <span aria-hidden="true">📋</span> {copyStatus === 'copied' ? 'Copied' : copyStatus === 'error' ? 'Copy failed' : 'Copy'}
            </button>
            {!live && (
              <>
                <NoteSyntaxButton
                  parserId={note.parser_id}
                  tips={vimEnabled ? [...EDITOR_TIPS, VIM_TIP] : EDITOR_TIPS}
                  onInsert={source => setDataContent(insertSource(dataEditorRef.current?.view, dataContent, source))}
                />
                <InsertAudioButton
                  parserId={note.parser_id}
                  onInsert={snippet => setDataContent(insertInline(dataEditorRef.current?.view, dataContent, snippet))}
                />
                <InsertImageOcclusionButton
                  parserId={note.parser_id}
                  onInsert={snippet => setDataContent(insertBlock(dataEditorRef.current?.view, dataContent, snippet))}
                />
              </>
            )}
          </div>
        </div>
        <CodeMirror
          ref={dataEditorRef}
          theme={theme}
          value={dataContent}
          onChange={setDataContent}
          // Still focusable, so it can be scrolled and moved around in, as with Vim's hjkl
          readOnly={live}
          extensions={dataExtensions}
          onCreateEditor={view => {
            // Vim's insert mode is the first sign of trying to edit
            if (live) getCM(view)?.on('vim-mode-change', (e: { mode: string }) => { if (e.mode === 'insert') setLiveEditTried(true); });
          }}
          basicSetup={{ lineNumbers: editor.lineNumbers }}
          style={cmStyle}
        />
      </div>

      <div className="form-grid">
        <div>
          <div style={fieldLabel}>Tags <span style={{ fontWeight: 400, textTransform: 'none', letterSpacing: 0 }}>(one per line)</span></div>
          <CodeMirror
            theme={theme}
            value={tagsContent}
            onChange={setTagsContent}
            extensions={editor.extensions}
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
            extensions={editor.extensions}
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

      {(imageOcclusions.length > 0 || imageOcclusionsError) && !isNarrow && (
        <div style={{ marginBottom: 16 }}>
          <div style={metaLabel}>Image Occlusions</div>
          {imageOcclusionsError && <div style={{ fontSize: 13, color: 'var(--error)' }}>{imageOcclusionsError}</div>}
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 4 }}>
            {imageOcclusions.map((occlusion, index) => (
              <button
                key={occlusion.clozes_filepath}
                onClick={() => setEditingOcclusion({ index, occlusion })}
                // The server edits the saved text, which would drop unsaved edits
                disabled={live || dataContent !== savedData}
                title={dataContent !== savedData ? 'Save or undo your changes to the data first' : occlusion.original_image_filepath}
                className="button-tool tool-purple"
                style={{ padding: '4px 8px', fontSize: 12 }}
              >
                <span aria-hidden="true">🖼️</span> Edit image occlusion {index + 1}
              </button>
            ))}
          </div>
        </div>
      )}
      {editingOcclusion && (
        <ImageOcclusionEditor
          mode="edit"
          occlusion={editingOcclusion.occlusion}
          onSave={svg => saveImageOcclusion(editingOcclusion.index, svg)}
          onClose={() => setEditingOcclusion(null)}
        />
      )}

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
                  className="button-link"
                  style={{ padding: '4px 10px', fontSize: 12 }}
                >
                  {ln.searched_keyword} ({id === null ? 'unmatched' : formatNoteId(id)})
                </button>
              );
            })}
          </div>
        </div>
      )}
      {onOpenNote && <Backlinks noteId={note.id} version={note.updated_at} onOpenNote={onOpenNote} />}
      {onOpenNote && localGraph && <LocalGraph noteId={note.id} onOpenNote={onOpenNote} />}

      <div>
        <div style={fieldLabel}>Compiled</div>
        <div style={{ border: '1px solid var(--border-subtle)', borderRadius: 4, padding: 4, backgroundColor: 'var(--bg)' }}>
          {render === null && <div>Loading…</div>}
          {render && 'error' in render && <div style={{ color: 'var(--error)', fontSize: 13 }}>{render.error}</div>}
          {render && 'result' in render && (
            <CardRenderer
              path={render.result.rendered_path}
              source={render.result.browser_source}
              webPath={render.result.web_path}
              onOpenNote={onOpenNote}
              version={render.version}
            />
          )}
        </div>
      </div>
    </div>
  );
}
