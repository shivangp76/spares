import { Link } from 'react-router-dom';
import { useServerReachable } from '../api/reachability';
import { useLocalStatus } from '../local/copy';
import { useCopyOwner } from '../local/transport';

/**
 * Whether the server can be reached. On a device that keeps a copy of the collection, also its
 * changes not yet synced, the files it is downloading, and why the copy can't be used, if it can't.
 */
export default function OfflineIndicator() {
  const online = useServerReachable();
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
    local.downloading && local.downloading.total > 0 && `Downloading files ${local.downloading.done}/${local.downloading.total}`,
  ].filter(Boolean);
  if (parts.length === 0) return null;
  const offlineTitle = local.active
    ? 'The server cannot be reached. This device has the whole collection, and sends its changes once it can.'
    : 'The server cannot be reached. To use the app without it, keep a copy of the collection on this device (Settings → This Device).';
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
