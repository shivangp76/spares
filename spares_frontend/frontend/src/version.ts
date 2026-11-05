// Which build of the app this page runs, and whether the server has a newer one.
//
// An installed web app, as on iOS, is often a page resumed from days ago rather than loaded again,
// so it can run an old build long after the server has a new one. The page asks the server which
// build it serves (`GET /api/version`) when it opens, when it is shown again, and every so often,
// and the navbar offers a reload once they differ (UpdateIndicator.tsx).
//
// The request goes to the server that served the page, which the service worker leaves to the
// network, as it does all of `/api/`.
import { useSyncExternalStore } from 'react';

/** This page's build, from `<meta name="spares-build">` (see build-id.ts), or null in development. */
export const PAGE_BUILD: string | null =
  document.querySelector<HTMLMetaElement>('meta[name="spares-build"]')?.content || null;

/** What `GET /api/version` answers (see `spares_server::version`). */
export interface ServerVersion {
  server: {
    version: string;
    /** When the server's binary was built, if known */
    built: string | null;
    started: string;
  };
  /** The build the server serves, or null if it serves none with an id */
  frontend: { build: string } | null;
}

// Often enough that a page left open picks up a deploy the same day
const CHECK_INTERVAL_MS = 30 * 60 * 1000;
// Showing the page again checks at most this often
const MIN_CHECK_GAP_MS = 60 * 1000;

let serverVersion: ServerVersion | null = null;
let lastCheck = 0;
const listeners = new Set<() => void>();

/** Asks the server which build it has, and the browser to look for a new service worker. Keeps the last answer when offline. */
export async function checkVersion(): Promise<ServerVersion | null> {
  lastCheck = Date.now();
  navigator.serviceWorker?.getRegistration().then(registration => registration?.update()).catch(() => {});
  try {
    const res = await fetch('/api/version', { cache: 'no-store' });
    if (!res.ok) return serverVersion;
    serverVersion = (await res.json()) as ServerVersion;
    listeners.forEach(listener => listener());
  } catch {
    // Offline
  }
  return serverVersion;
}

/** Checks now, whenever the page is shown or back online, and every so often. */
export function startVersionChecks(): void {
  const checkSoon = () => {
    if (document.visibilityState === 'visible' && Date.now() - lastCheck >= MIN_CHECK_GAP_MS) void checkVersion();
  };
  void checkVersion();
  document.addEventListener('visibilitychange', checkSoon);
  // A page restored from the back-forward cache
  window.addEventListener('pageshow', event => { if (event.persisted) checkSoon(); });
  window.addEventListener('online', checkSoon);
  setInterval(checkSoon, CHECK_INTERVAL_MS);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The server's last answer to `GET /api/version`, or null before one. */
export function useServerVersion(): ServerVersion | null {
  return useSyncExternalStore(subscribe, () => serverVersion);
}

/** Whether the server serves a build other than this page's. */
export function isOutdated(version: ServerVersion | null): boolean {
  const served = version?.frontend?.build;
  return PAGE_BUILD !== null && !!served && served !== PAGE_BUILD;
}

const INSTALL_WAIT_MS = 10_000;
const ACTIVATE_WAIT_MS = 3_000;

/**
 * Loads the server's build. The page is always fetched from the server when it can be, but a new
 * service worker only takes over once no page uses the old one, so it is told to take over first,
 * and keeps the new build for when the device is offline.
 */
export async function reloadToUpdate(): Promise<void> {
  try {
    const registration = await navigator.serviceWorker?.getRegistration();
    if (registration) {
      await registration.update().catch(() => {});
      const installing = registration.installing;
      if (installing) await withTimeout(untilInstalled(installing), INSTALL_WAIT_MS);
      if (registration.waiting) {
        const activated = new Promise<void>(resolve => navigator.serviceWorker.addEventListener('controllerchange', () => resolve(), { once: true }));
        registration.waiting.postMessage('skipWaiting');
        await withTimeout(activated, ACTIVATE_WAIT_MS);
      }
    }
  } finally {
    location.reload();
  }
}

function untilInstalled(worker: ServiceWorker): Promise<void> {
  return new Promise(resolve => {
    const check = () => { if (worker.state !== 'installing') resolve(); };
    worker.addEventListener('statechange', check);
    check();
  });
}

function withTimeout(promise: Promise<void>, ms: number): Promise<void> {
  return Promise.race([promise, new Promise<void>(resolve => setTimeout(resolve, ms))]);
}
