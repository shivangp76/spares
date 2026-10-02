import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { listParsers } from '../api/client';
import ImageOcclusionEditor from '../components/ImageOcclusionEditor';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { useMediaQuery } from '../hooks/useMediaQuery';
import type { ParserResponse } from '../types/spares';
import { sectionLabel } from '../utils';

// Enough to list every parser on one page
const PARSERS_LIMIT = 1000;

/** Makes image occlusions for notes written outside of the frontend, e.g. in a text editor and
    synced with the CLI. Notes edited here can insert them directly instead. */
export default function ImageOcclusionPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const isNarrow = useMediaQuery('(max-width: 640px)');
  const [parsers, setParsers] = useState<ParserResponse[] | null>(null);
  const [parserId, setParserId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  // Newest first
  const [snippets, setSnippets] = useState<{ parserName: string; snippet: string }[]>([]);
  const [copied, setCopied] = useState<number | null>(null);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    listParsers(1, PARSERS_LIMIT).then(
      list => { setParsers(list); setParserId(id => id ?? list[0]?.id ?? null); },
      e => setError(String(e)),
    );
  }, [credentials, navigate]);

  async function copy(index: number, snippet: string) {
    try {
      await navigator.clipboard.writeText(snippet);
      setCopied(index);
      setTimeout(() => setCopied(c => (c === index ? null : c)), 2000);
    } catch (e) {
      setError(String(e));
    }
  }

  const parserName = parsers?.find(p => p.id === parserId)?.name ?? '';

  return (
    <div className="page">
      <Navbar onLogout={logout} />
      <h2 style={{ marginBottom: 8 }}>Image Occlusion</h2>
      <p style={{ marginTop: 0, fontSize: 14, color: 'var(--text-secondary)' }}>
        Draw clozes over an image. The image and clozes are stored on the server, and you get a block to paste into a note.
        When editing a note here, use <em>Insert image occlusion</em> by its data instead.
      </p>

      {isNarrow ? (
        <p style={{ fontSize: 14 }}>The image occlusion editor needs a larger screen.</p>
      ) : (
        <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 16, flexWrap: 'wrap' }}>
          <label style={{ fontSize: 14 }}>
            Parser{' '}
            <select
              value={parserId ?? ''}
              onChange={e => setParserId(Number(e.target.value))}
              disabled={!parsers}
              style={{ padding: '6px 10px', fontSize: 14 }}
            >
              {!parsers && <option>Loading…</option>}
              {parsers?.map(p => <option key={p.id} value={p.id}>{p.name} ({p.id})</option>)}
            </select>
          </label>
          <button onClick={() => setEditorOpen(true)} disabled={parserId === null} style={{ padding: '6px 14px', fontSize: 14 }}>
            New image occlusion
          </button>
        </div>
      )}
      {error && <div style={{ fontSize: 13, color: 'var(--error)', marginBottom: 16 }}>{error}</div>}

      {snippets.map(({ parserName: name, snippet }, index) => (
        <div key={snippets.length - index} style={{ marginBottom: 16 }}>
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 8 }}>
            <div style={sectionLabel}>For {name}</div>
            <button onClick={() => copy(index, snippet)} style={{ padding: '2px 8px', fontSize: 12 }}>
              {copied === index ? 'Copied' : 'Copy'}
            </button>
          </div>
          <pre style={{ margin: 0, padding: 10, fontSize: 12, overflowX: 'auto', border: '1px solid var(--border-subtle)', borderRadius: 4, background: 'var(--surface)' }}>{snippet}</pre>
        </div>
      ))}

      {editorOpen && parserId !== null && (
        <ImageOcclusionEditor
          mode="new"
          parserId={parserId}
          onCreated={snippet => {
            setEditorOpen(false);
            setSnippets(prev => [{ parserName, snippet }, ...prev]);
            setCopied(c => (c === null ? null : c + 1));
          }}
          onClose={() => setEditorOpen(false)}
        />
      )}
    </div>
  );
}
