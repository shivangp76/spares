import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { getSyncStatus, isNetworkError, listSyncDevices, removeSyncDevice, type SyncDevice } from '../api/client';
import { disableLocalCopy, enableLocalCopy, useLocalStatus, type SetupStep } from '../local/copy';
import { canKeepCopy, getLocalCopy } from '../local/transport';
import { useOfflineStatus } from '../offline/review';
import { errorMessage, sectionLabel } from '../utils';

/** What the device is called in the server's list of devices, e.g. `iPhone (Firefox)`. */
function defaultDeviceName(): string {
  const ua = navigator.userAgent;
  const device = /iPhone/.test(ua) ? 'iPhone'
    : /iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1) ? 'iPad'
    : /Android/.test(ua) ? 'Android'
    : /Macintosh/.test(ua) ? 'Mac'
    : /Windows/.test(ua) ? 'Windows'
    : 'Linux';
  const browser = /FxiOS|Firefox/.test(ua) ? 'Firefox'
    : /EdgiOS|Edg\//.test(ua) ? 'Edge'
    : /CriOS|Chrome/.test(ua) ? 'Chrome'
    : 'Safari';
  return `${device} (${browser})`;
}

/** iOS deletes a site's storage after 7 days without a visit, except for apps on the Home Screen. */
function needsHomeScreen(): boolean {
  const ios = /iPhone|iPad/.test(navigator.userAgent) || (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1);
  const standalone = window.matchMedia('(display-mode: standalone)').matches || (navigator as { standalone?: boolean }).standalone === true;
  return ios && !standalone;
}

function formatBytes(bytes: number): string {
  return bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.round(bytes / 1e6)} MB`;
}

const muted = { fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' } as const;

/**
 * The devices enrolled with the sync server. A number stays taken until it is removed, even when
 * a browser deleted its copy, and there are only three. A laptop's own server is a sync client,
 * which has no list, so nothing is shown there.
 */
function DeviceList({ active, own }: { active: boolean; own: number | null }) {
  const [devices, setDevices] = useState<SyncDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  // Listed again once the server can be reached, e.g. after the page loaded offline
  const { online } = useOfflineStatus();
  const { lastSyncAt } = useLocalStatus();

  useEffect(() => {
    let cancelled = false;
    (async () => {
      // A device's copy answers the status as a client, but asks the server for the list
      if (!active && (await getSyncStatus()).role !== 'server') return null;
      return listSyncDevices();
    })().then(
      list => { if (!cancelled) { setDevices(list); setError(null); } },
      e => {
        if (cancelled) return;
        setError(isNetworkError(e)
          ? "Only the server has the list of devices, and it can't be reached. It shows once it can."
          : `Couldn't list the devices: ${errorMessage(e)}`);
      },
    );
    return () => { cancelled = true; };
  }, [active, own, version, online, lastSyncAt]);

  async function remove(device: SyncDevice) {
    if (!window.confirm(`Remove ${device.name} (replica ${device.replica})? It can no longer sync, and its changes not synced yet never reach the server. Its copy has to be set up again.`)) return;
    try {
      await removeSyncDevice(device.replica);
    } catch (e) {
      setError(`Couldn't remove ${device.name}: ${errorMessage(e)}`);
      return;
    }
    setVersion(v => v + 1);
  }

  if (error !== null) return <p style={muted}>{error}</p>;
  if (devices === null) return null;
  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ fontSize: 13, fontWeight: 600 }}>Devices keeping a copy</div>
      {devices.length === 0 && <p style={muted}>None.</p>}
      <ul style={{ listStyle: 'none', padding: 0, margin: '4px 0 0' }}>
        {devices.map(device => (
          <li key={device.replica} style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', padding: '4px 0' }}>
            <span>
              {device.name} <span style={{ color: 'var(--text-muted)' }}>(replica {device.replica})</span>
            </span>
            <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              {device.last_sync_at !== null ? `last synced ${new Date(device.last_sync_at * 1000).toLocaleString()}` : 'not synced since enrolling'}
            </span>
            {device.replica === own
              ? <span style={{ fontSize: 12, color: 'var(--text-secondary)' }}>This device</span>
              : <button onClick={() => remove(device)} style={{ fontSize: 12 }}>Remove</button>}
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Settings → This Device: keeping a copy of the collection here (see src/local/copy.ts). */
export default function LocalCopySettings() {
  const local = useLocalStatus();
  const copy = getLocalCopy();
  const [name, setName] = useState(defaultDeviceName);
  const [step, setStep] = useState<SetupStep | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [storage, setStorage] = useState<{ usage: number; persisted: boolean } | null>(null);
  // Until known, offered as if it can
  const [canKeep, setCanKeep] = useState(true);

  useEffect(() => {
    let cancelled = false;
    canKeepCopy().then(can => { if (!cancelled) setCanKeep(can); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (!local.active) return;
    Promise.all([navigator.storage.estimate(), navigator.storage.persisted?.() ?? Promise.resolve(false)])
      .then(([estimate, persisted]) => setStorage({ usage: estimate.usage ?? 0, persisted }))
      .catch(() => {});
  }, [local.active, local.lastSyncAt]);

  async function enable() {
    setError(null);
    setMessage(null);
    try {
      await enableLocalCopy(name.trim() || defaultDeviceName(), setStep);
      setMessage('This device now keeps a copy of the collection.');
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setStep(null);
    }
  }

  async function disable() {
    setError(null);
    const unsynced = local.dirty > 0 ? ` Its ${local.dirty} changes are sent to the server first.` : '';
    if (!window.confirm(`Stop keeping a copy on this device?${unsynced} Afterwards, this device works only while it can reach the server.`)) return;
    let warning: string | null;
    try {
      warning = await disableLocalCopy(false);
    } catch (e) {
      // The copy's sync errors already start with it
      const reason = errorMessage(e).replace(/^Sync failed: /, '');
      if (!window.confirm(`Syncing failed: ${reason}\n\nDelete the copy anyway, with the changes not yet sent?`)) return;
      try {
        warning = await disableLocalCopy(true);
      } catch (e2) {
        setError(errorMessage(e2));
        return;
      }
    }
    if (warning) window.alert(`The copy is deleted. ${warning}`);
    // Pages fetched from the copy fetch from the server again
    window.location.reload();
  }

  async function setUpAgain() {
    if (!window.confirm('Set up the copy again? Changes made here since the last sync are lost.')) return;
    try {
      await disableLocalCopy(true);
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    await enable();
  }

  const setUp = (
    <>
      <label style={{ display: 'flex', gap: 6, alignItems: 'center', marginTop: 8, flexWrap: 'wrap' }}>
        Name on the server:
        <input value={name} onChange={e => setName(e.target.value)} disabled={step !== null} style={{ width: 200 }} />
      </label>
      <div style={{ marginTop: 8, display: 'flex', gap: 12, alignItems: 'center', flexWrap: 'wrap' }}>
        <button onClick={enable} disabled={step !== null}>
          {copy && !copy.ready ? 'Finish Setting Up' : 'Keep a Copy on This Device'}
        </button>
        {step && <span role="status" style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{step}…</span>}
      </div>
    </>
  );

  let body;
  if (local.active && local.notEnrolled) {
    body = (
      <>
        <p style={{ margin: 0, color: 'var(--warning)' }}>The server no longer has this device, e.g. because it was removed there. Its copy has to be set up again.</p>
        <button onClick={setUpAgain} disabled={step !== null} style={{ marginTop: 8 }}>Set Up Again</button>
        {step && <span role="status" style={{ fontSize: 13, color: 'var(--text-secondary)', marginLeft: 12 }}>{step}…</span>}
      </>
    );
  } else if (local.active) {
    body = (
      <>
        <p style={{ margin: 0 }}>
          This device keeps a copy of the collection{local.replica !== null ? ` (replica ${local.replica})` : ''}
          {local.lastSyncAt ? `, last synced ${new Date(local.lastSyncAt * 1000).toLocaleString()}` : ''}.
          {local.dirty > 0 && ` ${local.dirty} changes are waiting to be sent.`}
          {local.conflicts > 0 && <> <Link to="/conflicts">{local.conflicts} conflicts</Link> need resolving.</>}
        </p>
        {storage && (
          <p style={muted}>
            Uses {formatBytes(storage.usage)} of this browser's storage.
            {!storage.persisted && ' The browser may delete it when storage runs low: sync before that can happen.'}
          </p>
        )}
        {needsHomeScreen() && <p style={{ ...muted, color: 'var(--warning)' }}>Use spares from the Home Screen: Safari deletes a website's storage after 7 days without a visit.</p>}
        <button onClick={disable} style={{ marginTop: 8 }}>Stop Keeping a Copy</button>
      </>
    );
  } else if (!canKeep) {
    body = <p style={{ margin: 0 }}>This browser can't keep a copy of the collection here, e.g. because private windows have no storage for it.</p>;
  } else {
    body = (
      <>
        <p style={{ margin: 0 }}>
          Keep the whole collection on this device, so that every page works offline: reviewing, searching, statistics, and adding and editing notes and tags.
          It syncs with the server when the app opens, when it is closed or idle, and with the Sync button.
        </p>
        <p style={muted}>Typst cards compile here. Other notes changed here are rendered by the server after the next sync; until then they show as they were, or as their text.</p>
        {needsHomeScreen() && <p style={{ ...muted, color: 'var(--warning)' }}>On iPhone and iPad, add spares to the Home Screen first and set this up there: Safari deletes a website's storage after 7 days without a visit.</p>}
        {setUp}
      </>
    );
  }

  return (
    <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
      <legend style={{ ...sectionLabel, padding: 0 }}>This Device</legend>
      {body}
      <DeviceList active={local.active} own={local.active ? local.replica : null} />
      {message && <p role="status" style={{ marginTop: 8, color: 'var(--text-secondary)' }}>{message}</p>}
      {error && <p role="alert" style={{ marginTop: 8, color: 'var(--error)' }}>{error}</p>}
    </fieldset>
  );
}
