import { useCallback, useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { listSyncConflicts, resolveSyncConflict, type ConflictResolution, type SyncConflict } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import { errorMessage, formatNoteId, sectionLabel } from '../utils';

const KIND_ABOUT: Record<SyncConflict['kind'], string> = {
  text: 'Both sides changed the same lines. This side’s text was kept.',
  deleted_there: 'The other side deleted the note, which was edited here. It was kept.',
  deleted_here: 'This side deleted the note, which the other side edited. It came back with that side’s text.',
};

const textBox: React.CSSProperties = {
  margin: 0,
  padding: 8,
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-word',
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
  fontSize: 13,
  background: 'var(--surface)',
  borderRadius: 4,
  maxHeight: 320,
  overflow: 'auto',
};

function Side({ label, text }: { label: string; text: string | null }) {
  return (
    <div style={{ flex: '1 1 280px', minWidth: 0 }}>
      <div style={sectionLabel}>{label}</div>
      {text === null ? <p style={{ margin: 0, color: 'var(--text-muted)' }}>Deleted</p> : <pre style={textBox}>{text}</pre>}
    </div>
  );
}

function Conflict({ conflict, onResolve }: { conflict: SyncConflict; onResolve: (resolution: ConflictResolution) => Promise<void> }) {
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const noteId = formatNoteId(conflict.note_id);

  async function resolve(resolution: ConflictResolution) {
    setBusy(true);
    try {
      await onResolve(resolution);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section style={{ borderTop: '1px solid var(--border)', padding: '12px 0' }}>
      <h3 style={{ margin: '0 0 4px' }}>
        Note <Link to={`/notes?q=${encodeURIComponent(`id=${noteId}`)}`}>{noteId}</Link>
      </h3>
      <p style={{ margin: '0 0 8px', fontSize: 13, color: 'var(--text-secondary)' }}>
        {KIND_ABOUT[conflict.kind]} Found {new Date(conflict.created_at * 1000).toLocaleString()}.
      </p>
      {editing === null ? (
        <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
          <Side label="This side (now)" text={conflict.local} />
          <Side label="Other side" text={conflict.remote} />
        </div>
      ) : (
        <textarea value={editing} onChange={e => setEditing(e.target.value)} rows={12} style={{ width: '100%', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: 13 }} />
      )}
      {conflict.base !== null && editing === null && (
        <details style={{ marginTop: 8 }}>
          <summary style={{ cursor: 'pointer', fontSize: 13 }}>Text at the last sync</summary>
          <pre style={{ ...textBox, marginTop: 4 }}>{conflict.base}</pre>
        </details>
      )}
      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 8 }}>
        {editing !== null ? (
          <>
            <button onClick={() => resolve({ text: editing })} disabled={busy}>Save This Text</button>
            <button onClick={() => setEditing(null)} disabled={busy}>Cancel</button>
          </>
        ) : (
          <>
            {conflict.local !== null && <button onClick={() => resolve('keep')} disabled={busy}>Keep This Side</button>}
            {conflict.local !== null && conflict.remote !== null && (
              <button onClick={() => resolve({ text: conflict.remote! })} disabled={busy}>Use the Other Side</button>
            )}
            {conflict.local !== null && <button onClick={() => setEditing(conflict.local)} disabled={busy}>Edit…</button>}
            <button onClick={() => resolve(conflict.local === null ? 'keep' : 'delete')} disabled={busy}>
              {conflict.local === null ? 'Leave It Deleted' : 'Delete the Note'}
            </button>
          </>
        )}
      </div>
    </section>
  );
}

/** `spares sync conflicts`: notes a sync could not merge on its own, on the replica that found them. */
export default function ConflictsPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const [conflicts, setConflicts] = useState<SyncConflict[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const load = useCallback(() => {
    listSyncConflicts().then(setConflicts, e => setError(errorMessage(e)));
  }, []);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    load();
  }, [credentials, navigate, load]);

  async function resolve(conflict: SyncConflict, resolution: ConflictResolution) {
    try {
      await resolveSyncConflict(conflict.id, resolution);
      setStatus(`Resolved the conflict of note ${formatNoteId(conflict.note_id)}. The next sync sends it.`);
      load();
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  return (
    <div className="page">
      <h2 style={{ marginBottom: 8 }}>Conflicts</h2>
      <p style={{ marginTop: 0, color: 'var(--text-secondary)' }}>
        Notes a sync could not merge on its own. Card ids stand in for card orders (o:c40 is card 40), so the same card reads the same on both sides.
      </p>
      {status && <p role="status" style={{ color: 'var(--text-secondary)' }}>{status}</p>}
      {error && <p role="alert" style={{ color: 'var(--error)' }}>{error}</p>}
      {conflicts === null && !error && <p className="loading-delayed">Loading…</p>}
      {conflicts?.length === 0 && <p>No conflicts.</p>}
      {conflicts?.map(conflict => <Conflict key={conflict.id} conflict={conflict} onResolve={resolution => resolve(conflict, resolution)} />)}
    </div>
  );
}
