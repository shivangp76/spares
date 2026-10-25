import { Link } from 'react-router-dom';
import { useLocalStatus } from '../local/copy';
import { useCopyOwner } from '../local/transport';
import { useOfflineStatus } from '../offline/review';

/**
 * Whether the server can be reached, and what is waiting to be sent to it or downloaded from it.
 * On a device that keeps a copy of the collection, its changes not yet synced, and why the copy
 * can't be used, if it can't.
 */
export default function OfflineIndicator() {
  const { online, pending, downloading } = useOfflineStatus();
  const local = useLocalStatus();
  const owner = useCopyOwner();
  if (local.active && owner === 'elsewhere') {
    return <span role="status" style={{ fontSize: 12, color: 'var(--warning)', alignSelf: 'center' }}>Open in another tab: close it to use this one</span>;
  }
  if (local.active && local.notEnrolled) {
    return <Link to="/settings" role="status" style={{ fontSize: 12, color: 'var(--warning)', alignSelf: 'center' }}>Removed from the server: set the copy up again</Link>;
  }
  const parts = [
    !online && 'Offline',
    local.active && local.dirty > 0 && `${local.dirty} unsynced`,
    !local.active && pending > 0 && `${pending} to sync`,
    local.downloading && local.downloading.total > 0 && `Downloading files ${local.downloading.done}/${local.downloading.total}`,
    downloading && `Downloading cards ${downloading.done}/${downloading.total}`,
  ].filter(Boolean);
  if (parts.length === 0) return null;
  const offlineTitle = local.active
    ? 'The server cannot be reached. This device has the whole collection, and sends its changes once it can.'
    : 'The server cannot be reached. Downloaded cards can still be reviewed, and ratings are sent once it can.';
  return (
    <span
      role="status"
      title={online ? undefined : offlineTitle}
      style={{ fontSize: 12, color: online ? 'var(--text-secondary)' : 'var(--warning)', alignSelf: 'center', whiteSpace: 'nowrap' }}
    >
      {parts.join(' · ')}
    </span>
  );
}
