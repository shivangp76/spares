import { useEffect, useState } from 'react';
import { getSyncStatus, runSync, type SyncStatus } from '../api/client';
import { formatNoteId } from '../utils';

/** How long ago `unixSeconds` was, e.g. `5 min ago`. */
function ago(unixSeconds: number): string {
  const minutes = Math.round((Date.now() / 1000 - unixSeconds) / 60);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} days ago`;
}

/**
 * Syncs the laptop's server with the sync server (`spares sync cloud`), with the time of the last
 * sync and the number of unresolved conflicts. Shown only where there is something to sync with.
 */
export default function SyncButton({ onStatus }: { onStatus: (message: string) => void }) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [syncing, setSyncing] = useState(false);

  useEffect(() => {
    // Older servers and offline clients have no status; the button just stays hidden
    getSyncStatus().then(setStatus, () => setStatus(null));
  }, []);

  if (!status || status.role !== 'client' || !status.sync_url) return null;

  async function sync() {
    setSyncing(true);
    try {
      const report = await runSync();
      const conflicts = report.conflicts.length > 0
        ? ` ${report.conflicts.length} conflicts (${report.conflicts.map(formatNoteId).join(', ')}): resolve them with \`spares sync conflicts\`.`
        : '';
      onStatus(report.mode === null
        ? `Already in sync.${conflicts}`
        : `Synced: received ${report.pulled} changes, sent ${report.pushed}.${conflicts}`);
    } catch (e) {
      const message = String(e);
      onStatus(message.includes('full sync')
        ? `${message} Run \`spares sync cloud\` to choose which side to keep.`
        : message);
    } finally {
      setSyncing(false);
      getSyncStatus().then(setStatus, () => {});
    }
  }

  const title = [
    status.last_sync_at ? `Last synced ${ago(status.last_sync_at)}` : 'Never synced',
    status.conflicts > 0 ? `${status.conflicts} unresolved conflicts` : null,
    status.needs_full_sync ? `Needs a full sync: ${status.needs_full_sync}` : null,
  ].filter(Boolean).join('. ');

  return (
    <button onClick={sync} disabled={syncing} className="navbar-button" title={title}>
      {syncing ? 'Syncing…' : 'Sync'}
      {status.last_sync_at && !syncing && <span style={{ fontSize: 11, color: 'var(--text-muted)', marginLeft: 4 }}>{ago(status.last_sync_at)}</span>}
      {status.conflicts > 0 && <span className="sync-conflict-badge" style={{ marginLeft: 4 }}>{status.conflicts}</span>}
    </button>
  );
}
