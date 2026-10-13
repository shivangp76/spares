import { useOfflineStatus } from '../offline/review';

/** Whether the server can be reached, and what is waiting to be sent to it or downloaded from it. */
export default function OfflineIndicator() {
  const { online, pending, downloading } = useOfflineStatus();
  const parts = [
    !online && 'Offline',
    pending > 0 && `${pending} to sync`,
    downloading && `Downloading cards ${downloading.done}/${downloading.total}`,
  ].filter(Boolean);
  if (parts.length === 0) return null;
  return (
    <span
      role="status"
      title={online ? undefined : 'The server cannot be reached. Downloaded cards can still be reviewed, and ratings are sent once it can.'}
      style={{ fontSize: 12, color: online ? 'var(--text-secondary)' : 'var(--warning)', alignSelf: 'center', whiteSpace: 'nowrap' }}
    >
      {parts.join(' · ')}
    </span>
  );
}
