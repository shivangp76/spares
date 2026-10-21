import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { getNotesVcsStatus, removeSavedCopies, type NotesVcsStatus } from '../api/client';
import { docUrl } from '../help/pages';
import { errorMessage } from '../utils';
import HelpDialog from './HelpDialog';

const MANUAL = { label: 'Files spares overwrote', file: 'workflows.md', heading: 'Files spares overwrote' };

/**
 * Copies of files spares overwrote in its notes repository without having written them, which may
 * hold edits that are nowhere else. Shown until the user deletes them, since spares never does.
 */
export default function SavedCopiesNotice() {
  const [status, setStatus] = useState<NotesVcsStatus | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Errors are ignored: this is a notice, and the server may be offline
  const refresh = useCallback(() => {
    getNotesVcsStatus().then(setStatus, () => {});
  }, []);
  // A push or a reconcile can save copies at any time, so look again when the tab is returned to
  useEffect(() => {
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [refresh]);

  const copies = status?.saved_copies ?? [];
  if (copies.length === 0) return null;
  const differing = copies.filter(c => !c.matches_current).length;

  async function remove() {
    const question = differing > 0
      ? `Delete all ${copies.length} saved copies? ${differing} differ from the current file, so may hold edits that are nowhere else.`
      : `Delete all ${copies.length} saved copies? Each matches the current file.`;
    if (!window.confirm(question)) return;
    try {
      await removeSavedCopies(copies.map(c => c.path));
      setOpen(false);
      setError(null);
      refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <>
      <button
        onClick={() => setOpen(true)}
        className="navbar-button"
        style={{ color: 'var(--warning)' }}
        title="Spares overwrote files in its notes repository that it did not write, and saved copies of them"
      >
        Overwritten files saved ({copies.length})
      </button>
      {open && (
        <HelpDialog
          title="Overwritten files saved"
          wide
          onClose={() => setOpen(false)}
          intro={<>
            Spares overwrote or removed these files in its notes repository, which it did not write
            (for example, edits made there by hand instead of in your clone), and saved copies of
            them in <code>{status?.overwritten_dir}</code>. See <Link to={docUrl(MANUAL)}>{MANUAL.label}</Link>.
          </>}
        >
          <ul style={{ fontSize: 14, paddingLeft: 18 }}>
            {copies.map(c => (
              <li key={c.path}>
                <code>{c.path}</code>{' '}
                {c.matches_current
                  ? <span style={{ color: 'var(--text-secondary)' }}>same as the current file, so nothing is lost</span>
                  : <span style={{ color: 'var(--warning)' }}>differs from the current file: check it for edits to keep</span>}
              </li>
            ))}
          </ul>
          <p style={{ fontSize: 14 }}>
            To keep an edit from a copy, copy it into your clone and push it. Once you have checked
            them, delete the copies.
          </p>
          {error && <p style={{ fontSize: 14, color: 'var(--error)' }}>{error}</p>}
          <button onClick={remove}>Delete these copies</button>
        </HelpDialog>
      )}
    </>
  );
}
