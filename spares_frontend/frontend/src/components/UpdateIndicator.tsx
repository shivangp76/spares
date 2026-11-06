import { useState } from 'react';
import { isOutdated, PAGE_BUILD, reloadToUpdate, useServerVersion } from '../version';

/** Offers a reload once the server serves a newer build of the app than this page's (see src/version.ts). */
export default function UpdateIndicator() {
  const version = useServerVersion();
  const [reloading, setReloading] = useState(false);
  if (!isOutdated(version)) return null;
  return (
    <button
      onClick={() => { setReloading(true); void reloadToUpdate(); }}
      disabled={reloading}
      className="navbar-button"
      title={`This page runs build ${PAGE_BUILD}, and the server has ${version?.frontend?.build}. Reloading loads the server's.`}
      style={{ color: 'var(--warning)', borderColor: 'var(--warning)' }}
    >
      {reloading ? 'Updating…' : 'Update: reload'}
    </button>
  );
}
