// This device's copy of the collection (Settings → This Device): setting it up, syncing it with the
// server, and downloading the files it shows offline.
//
// The copy is a sync replica of its own, like the laptop (see `spares_core::sync`): every page
// works on it, offline too, and a sync sends its changes to the server and brings the server's.
//
// **When it syncs.** An undo can't reach back past the last sync, so the copy doesn't sync right
// after each action. It syncs when the app opens, when it is hidden or the connection comes back,
// after `IDLE_MS` without changes, and with the Sync button. Syncing when hidden only happens if
// something changed or the last sync is `PULL_EVERY_MS` old.
//
// **Files.** The server renders notes, and the device keeps every rendered file and every file
// Typst cards read (`render_asset`) in Cache Storage, where the API client looks first. They are
// downloaded again when they change, after each sync. So is pandoc.wasm, from the app's own
// build, which converts the Markdown cards of notes changed here (see pandoc.ts).
//
// **Logging in and out.** Everything the copy sends goes to its own server, with the API key only
// when the credentials are for that server. On every change of credentials, the copy pauses: no
// syncs or downloads in the background, while the copy and its files stay. It starts again once
// logged in to its server.
import { useSyncExternalStore } from "react";
import {
  clearCaches,
  fileUrl,
  getCredentials,
  listRenderPackages,
  onCredentialsChange,
  renderAssetUrl,
  reportServerReachable,
  type SyncReport,
} from "../api/client";
import {
  call,
  canKeepCopy,
  COPY_FILES_CACHE,
  COPY_RENDER_ASSETS_CACHE,
  getLocalCopy,
  LocalError,
  onLocalWrite,
  openCopy,
  releaseCopy,
  resumeCopy,
  setLocalCopy,
  stopCopy,
  usesLocalCopy,
} from "./transport";
import { COPY_PANDOC_CACHE, PANDOC_WASM_URL } from "./pandoc";

/** Syncs once nothing changed for this long */
const IDLE_MS = 5 * 60 * 1000;
/** Syncs even without changes of its own when the last sync is this old, to get the server's */
const PULL_EVERY_MS = 15 * 60 * 1000;
const CHECK_EVERY_MS = 60 * 1000;
/** How long a sync keeps trying while the server is busy, e.g. with a full upload's files */
const BUSY_RETRY_MS = 10 * 60 * 1000;
const BUSY_WAIT_MS = 15 * 1000;
/** Files downloaded at once */
const DOWNLOAD_CONCURRENCY = 4;

// What has been downloaded, by path: a rendered file's mtime, and a render asset's hash. Only
// `dropCopy` removes them, along with the caches they describe.
const FILES_KEY = "spares_copy_files";
const ASSETS_KEY = "spares_copy_assets";
// Tells the service worker to keep spares_wasm and the Typst compiler offline too (see service-worker.js)
const SERVICE_WORKER_MARKER = "spares-local-copy";

/** spares_wasm's `status()` */
interface CopyState {
  replica: number | null;
  dirty: number;
  last_sync_at: number | null;
  needs_full_sync: string | null;
  conflicts: number;
}

export interface LocalStatus {
  /** Whether requests are answered by this device's copy */
  active: boolean;
  replica: number | null;
  /** Rows changed here since the last sync */
  dirty: number;
  /** Unix time */
  lastSyncAt: number | null;
  conflicts: number;
  syncing: boolean;
  /** Why the last sync failed, if it did */
  error: string | null;
  /** The next sync has to replace one side's collection with the other's, which the user chooses */
  fullSyncRequired: boolean;
  /** The server no longer has this device enrolled, so the copy has to be set up again */
  notEnrolled: boolean;
  /** Files being downloaded */
  downloading: { done: number; total: number } | null;
  /** Notes changed here that the server hasn't rendered yet */
  pendingNotes: ReadonlySet<number>;
}

const OFF: LocalStatus = {
  active: false,
  replica: null,
  dirty: 0,
  lastSyncAt: null,
  conflicts: 0,
  syncing: false,
  error: null,
  fullSyncRequired: false,
  notEnrolled: false,
  downloading: null,
  pendingNotes: new Set(),
};

let status = OFF;
const listeners = new Set<() => void>();

function setStatus(update: Partial<LocalStatus>): void {
  status = { ...status, ...update };
  for (const listener of listeners) listener();
}

export function useLocalStatus(): LocalStatus {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => status,
  );
}

/** Whether the server hasn't rendered the note's latest change yet, made on this device. */
export function usePendingRender(noteId: number | undefined): boolean {
  const { pendingNotes } = useLocalStatus();
  return noteId !== undefined && pendingNotes.has(noteId);
}

/**
 * The Image Occlusion page's message on a device that uses its copy. The device uploads an image
 * occlusion once one of its notes names it (see `spares_wasm::media`), so one made for a note
 * written elsewhere would never reach the server.
 */
export const OCCLUSIONS_FROM_NOTES =
  "On a device that keeps a copy of the collection, insert image occlusions from a note's editor: the device sends them to the server with the note, at the next sync.";

function currentServer(): { serverUrl: string; apiKey: string } | null {
  try {
    return getCredentials();
  } catch {
    return null;
  }
}

/** The copy's server, with the API key when logged in to it. Everything the copy sends goes there. */
function copyServer(): { serverUrl: string; apiKey: string | null } | null {
  const copy = getLocalCopy();
  if (!copy) return null;
  const server = currentServer();
  return {
    serverUrl: copy.serverUrl,
    apiKey: server?.serverUrl === copy.serverUrl ? server.apiKey : null,
  };
}

/** `copyServer()`, throwing unless logged in to it. */
function copyCredentials(): { serverUrl: string; apiKey: string } {
  const server = copyServer();
  if (!server) throw new Error("This device doesn't keep a copy.");
  const { serverUrl, apiKey } = server;
  if (apiKey === null)
    throw new Error(`Log in to ${serverUrl} to sync this device's copy.`);
  return { serverUrl, apiKey };
}

async function refreshStatus(): Promise<void> {
  const [state, pending] = await Promise.all([
    call<CopyState>("status"),
    call<BigInt64Array>("pending_notes"),
  ]);
  setStatus({
    replica: state.replica,
    dirty: state.dirty,
    lastSyncAt: state.last_sync_at,
    conflicts: state.conflicts,
    fullSyncRequired: status.fullSyncRequired || state.needs_full_sync !== null,
    pendingNotes: new Set(Array.from(pending, Number)),
  });
}

// Syncing

let syncing: Promise<SyncReport> | null = null;
let lastWriteAt = 0;

/**
 * Syncs the copy with the server. `full_download` replaces the copy with the server's collection,
 * dropping its changes, and `full_upload` the server's with the copy, which makes every other
 * replica download it. One sync runs at a time.
 */
export function syncLocalCopy(
  mode: "normal" | "full_download" | "full_upload" = "normal",
): Promise<SyncReport> {
  syncing ??= (async () => {
    // Inside the `try`, so a failure is never left in `syncing` for later syncs to return
    try {
      const server = copyCredentials();
      setStatus({ syncing: true });
      const report = await syncWhileBusy(server, mode);
      reportServerReachable(true);
      setStatus({ error: null, fullSyncRequired: false, notEnrolled: false });
      // Pages show what they fetch next from the copy as it is now
      if (report.mode !== null) clearCaches();
      refreshFiles().catch((e) => console.error("Downloading files", e));
      return report;
    } catch (e) {
      const local = e instanceof LocalError ? e : null;
      // As a request through the API client does, for the navbar
      if (local?.unreachable) reportServerReachable(false);
      setStatus({
        error: e instanceof Error ? e.message : String(e),
        fullSyncRequired: local?.fullSyncRequired ?? false,
        notEnrolled: local?.notEnrolled ?? false,
      });
      throw e;
    } finally {
      setStatus({ syncing: false });
      await refreshStatus().catch(console.error);
      syncing = null;
    }
  })();
  return syncing;
}

/**
 * The copy's `sync`, tried again while the server is busy with another change: after a full
 * upload, the server regenerates every note's files, which can take minutes, and refuses syncs
 * meanwhile.
 */
async function syncWhileBusy(
  server: { serverUrl: string; apiKey: string },
  mode: "normal" | "full_download" | "full_upload",
): Promise<SyncReport> {
  const started = Date.now();
  for (;;) {
    try {
      return await call<SyncReport>(
        "sync",
        server.serverUrl,
        server.apiKey,
        mode,
      );
    } catch (e) {
      if (
        !(e instanceof LocalError && e.busy) ||
        Date.now() - started > BUSY_RETRY_MS
      )
        throw e;
    }
    await new Promise((resolve) => setTimeout(resolve, BUSY_WAIT_MS));
  }
}

function syncInBackground(): void {
  // Logged out, there is nothing to sync with, and no error worth showing
  if (
    !status.active ||
    status.fullSyncRequired ||
    status.notEnrolled ||
    !copyServer()?.apiKey
  )
    return;
  syncLocalCopy().catch(() => {
    // Kept in `status.error`, for the Sync button
  });
}

/** Whether there is something to sync: changes here, or the server's since long ago. */
function worthSyncing(now: number): boolean {
  return (
    status.dirty > 0 ||
    status.lastSyncAt === null ||
    now - status.lastSyncAt * 1000 >= PULL_EVERY_MS
  );
}

let listening = false;

/** Syncs when the app is hidden, when the connection comes back, and when idle (see the top). Once per page. */
function syncWhenDue(): void {
  if (listening) return;
  listening = true;
  let refresh: ReturnType<typeof setTimeout> | undefined;
  onLocalWrite(() => {
    lastWriteAt = Date.now();
    // Once for a burst of changes, e.g. a rating and the next card's fetch
    clearTimeout(refresh);
    refresh = setTimeout(() => {
      if (status.active) refreshStatus().catch(console.error);
    }, 300);
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden" && worthSyncing(Date.now()))
      syncInBackground();
  });
  window.addEventListener("online", syncInBackground);
  setInterval(() => {
    const now = Date.now();
    if (
      document.visibilityState === "visible" &&
      now - lastWriteAt >= IDLE_MS &&
      worthSyncing(now)
    )
      syncInBackground();
  }, CHECK_EVERY_MS);
}

/** Opens the copy if this device keeps one for the server logged in to, syncs it, and keeps syncing it in the background. */
export async function startLocalCopy(): Promise<void> {
  const server = currentServer();
  if (status.active || !server || !usesLocalCopy(server.serverUrl)) return;
  setStatus({ active: true });
  // In case the service worker was installed before the copy was set up
  navigator.serviceWorker?.controller?.postMessage("cacheLocalCopy");
  if (!(await openLocalCopy())) return;
  syncWhenDue();
  syncInBackground();
}

/** Opens the copy, and shows its status. False if it can't be opened. */
async function openLocalCopy(): Promise<boolean> {
  try {
    await openCopy();
    await refreshStatus();
  } catch (e) {
    // E.g. another tab has it open, which `useCopyOwner` shows
    setStatus({ error: e instanceof Error ? e.message : String(e) });
    return false;
  }
  // From an attempt that failed before
  setStatus({ error: null });
  return true;
}

// A page kept in the back-forward cache, e.g. on going to another site in the same tab, closes the
// copy, so the page that replaces it can open it (see `releaseCopy`). Shown again, as on going
// back, it opens the copy again, and syncs if the copy is due as on opening the app.
window.addEventListener("pagehide", (event) => {
  if (event.persisted && status.active) releaseCopy();
});
window.addEventListener("pageshow", (event) => {
  if (!event.persisted || !status.active) return;
  resumeCopy();
  openLocalCopy().then((opened) => {
    if (opened && worthSyncing(Date.now())) syncInBackground();
  }, console.error);
});

// Bumped when the copy pauses, which stops a download that is running
let session = 0;

onCredentialsChange(() => {
  session += 1;
  setStatus({ active: false, downloading: null });
  startLocalCopy().catch(console.error);
});

// Setting up and removing the copy

/** A step of `enableLocalCopy`, for its progress. */
export type SetupStep =
  | "Registering this device"
  | "Downloading the collection"
  | "Downloading files";

/**
 * Makes this browser keep a copy of the collection: registers it with the server as a replica
 * named `name`, downloads the collection and its files, and from then on answers requests from
 * the copy. Retrying after a failure carries on where it stopped.
 */
export async function enableLocalCopy(
  name: string,
  onStep: (step: SetupStep) => void,
): Promise<void> {
  if (!canKeepCopy())
    throw new Error(
      "This browser has no storage for a copy, as in a private window.",
    );
  const server = currentServer();
  if (!server) throw new Error("Log in first.");
  const { serverUrl, apiKey } = server;
  const existing = getLocalCopy();
  if (existing && existing.serverUrl !== serverUrl) {
    throw new Error(
      `This device keeps a copy for ${existing.serverUrl}. Log in to it to turn the copy off first.`,
    );
  }
  onStep("Registering this device");
  let state = (await openCopy()) as CopyState;
  if (state.replica !== null && existing === null) {
    // Left from a copy this page no longer knows of, e.g. after its site data was partly cleared.
    // Its number may be another device's by now, so it isn't unenrolled: if it is still this
    // browser's, it stays in the list of devices (`DeviceList`), to remove there.
    await call("forget");
    stopCopy();
    state = (await openCopy()) as CopyState;
  }
  // Already enrolled when an earlier attempt stopped after it
  const replica =
    state.replica ?? (await call<number>("enroll", serverUrl, apiKey, name));
  setLocalCopy({ serverUrl, replica, ready: false });
  onStep("Downloading the collection");
  await call<SyncReport>("sync", serverUrl, apiKey, "full_download");
  setLocalCopy({ serverUrl, replica, ready: true });
  clearCaches();
  await globalThis.caches
    ?.open(SERVICE_WORKER_MARKER)
    .then((cache) => cache.put("/", new Response("1")))
    .catch(console.error);
  navigator.serviceWorker?.controller?.postMessage("cacheLocalCopy");
  // That the browser keep it when storage runs low. Not waited for: Firefox asks the user first.
  navigator.storage.persist?.().catch(console.error);
  await startLocalCopy();
  onStep("Downloading files");
  await refreshFiles();
}

/** Forgets the copy here: the page goes back to asking the server. */
async function dropCopy(): Promise<void> {
  setLocalCopy(null);
  stopCopy();
  session += 1;
  localStorage.removeItem(FILES_KEY);
  localStorage.removeItem(ASSETS_KEY);
  for (const name of [
    SERVICE_WORKER_MARKER,
    COPY_FILES_CACHE,
    COPY_RENDER_ASSETS_CACHE,
    COPY_PANDOC_CACHE,
  ]) {
    await globalThis.caches?.delete(name).catch(console.error);
  }
  clearCaches();
  status = OFF;
  for (const listener of listeners) listener();
}

/**
 * Stops keeping a copy: syncs it first, unless `discardChanges`, then frees the device's replica
 * number on the server if it can, and deletes the copy either way. Throws if the sync fails, so
 * the user can choose to discard what it would have sent. Returns what to tell the user when the
 * server couldn't be told, e.g. offline.
 */
export async function disableLocalCopy(
  discardChanges: boolean,
): Promise<string | null> {
  if (!discardChanges && !status.notEnrolled) await syncLocalCopy();
  const replica = getLocalCopy()?.replica ?? status.replica;
  let warning: string | null = null;
  if (!status.notEnrolled) {
    try {
      const { serverUrl, apiKey } = copyCredentials();
      // Which deletes the copy too
      await call("unenroll", serverUrl, apiKey);
      await dropCopy();
      return null;
    } catch (e) {
      // Unless the server forgot the device already. Not tried again later: by then the number
      // may be another device's.
      if (!(e instanceof LocalError && e.notEnrolled)) {
        warning = `The server still lists this device as replica ${replica}. Remove it under Settings → This Device once online, or with \`spares sync devices remove ${replica}\`.`;
      }
    }
  }
  await call("forget");
  await dropCopy();
  return warning;
}

// Files

/** `GET /api/sync/files` */
interface RenderedFile {
  path: string;
  size: number;
  mtime: number;
}

function readMap(key: string): Record<string, string | number> {
  try {
    return JSON.parse(localStorage.getItem(key) ?? "{}");
  } catch {
    return {};
  }
}

/** Typst's package registry, as the compiler fetches from it (see src/typst/compiler.ts) */
function packageUrl(spec: string): string {
  const [namespace, name, version] = spec.split("/");
  return `https://packages.typst.org/${namespace}/${name}-${version}.tar.gz`;
}

let downloading: Promise<void> | null = null;

/**
 * Downloads the rendered files and render assets that changed since the last time, and drops the
 * ones that are gone, and pandoc.wasm if it isn't here. Typst packages are fetched too, which the
 * service worker keeps.
 */
export function refreshFiles(): Promise<void> {
  downloading ??= (async () => {
    try {
      // Again when the copy paused meanwhile and started again, e.g. on logging back in
      let started;
      do {
        started = session;
        await downloadFiles();
      } while (session !== started && status.active);
    } finally {
      downloading = null;
      setStatus({ downloading: null });
    }
  })();
  return downloading;
}

async function downloadFiles(): Promise<void> {
  if (typeof caches === "undefined") return;
  const started = session;
  const { serverUrl, apiKey } = copyCredentials();
  const headers = { Authorization: `Bearer ${apiKey}` };
  const res = await fetch(`${serverUrl}/api/sync/files`, { headers });
  if (!res.ok)
    throw new Error(`Listing the server's files failed: ${res.status}`);
  const files = (await res.json()) as RenderedFile[];
  const assets = await call<{ path: string; hash: string }[]>("render_assets");

  const [filesCache, assetsCache, pandocCache] = await Promise.all([
    caches.open(COPY_FILES_CACHE),
    caches.open(COPY_RENDER_ASSETS_CACHE),
    caches.open(COPY_PANDOC_CACHE),
  ]);
  const knownFiles = readMap(FILES_KEY);
  const knownAssets = readMap(ASSETS_KEY);
  const current = new Set(files.map((file) => file.path));
  for (const path of Object.keys(knownFiles)) {
    if (current.has(path)) continue;
    await filesCache.delete(fileUrl(path, serverUrl));
    delete knownFiles[path];
  }
  // Also those the device no longer needs, such as images of cards' image occlusions, which it makes itself
  const currentAssets = new Set(assets.map((asset) => asset.path));
  // For the Typst compiler, which otherwise keeps what it read of them while the page is open
  const changedAssets = new Set<string>();
  for (const path of Object.keys(knownAssets)) {
    if (currentAssets.has(path)) continue;
    await assetsCache.delete(renderAssetUrl(path, serverUrl));
    delete knownAssets[path];
    changedAssets.add(path);
  }
  const save = () => {
    localStorage.setItem(FILES_KEY, JSON.stringify(knownFiles));
    localStorage.setItem(ASSETS_KEY, JSON.stringify(knownAssets));
  };
  save();

  const tasks = [
    ...files
      .filter((file) => knownFiles[file.path] !== file.mtime)
      .map((file) => async () => {
        const url = fileUrl(file.path, serverUrl);
        const response = await fetch(url, { headers, cache: "no-cache" });
        // A file that went away meanwhile is left for the next time
        if (!response.ok) return;
        await filesCache.put(url, response);
        knownFiles[file.path] = file.mtime;
      }),
    ...assets
      .filter((asset) => knownAssets[asset.path] !== asset.hash)
      .map((asset) => async () => {
        const url = renderAssetUrl(asset.path, serverUrl);
        const response = await fetch(url, { headers, cache: "no-cache" });
        if (!response.ok && response.status !== 404) return;
        // A 404 is fetched again at the next refresh: the server may only be missing the file for
        // now, as when `spares sync cloud` has sent the database and not yet the files. Meanwhile
        // the version the device has, if any, is kept. Otherwise the 404 is, so that offline the
        // file isn't asked for.
        if (response.ok) {
          await assetsCache.put(url, response);
          knownAssets[asset.path] = asset.hash;
          changedAssets.add(asset.path);
        } else if (!(await assetsCache.match(url, { ignoreVary: true }))) {
          await assetsCache.put(url, response);
        }
      }),
  ];
  // The build's, which names its version. An earlier build's goes once this one's is here.
  const pandocWasms = await pandocCache.keys();
  if (!pandocWasms.some((request) => request.url === PANDOC_WASM_URL)) {
    tasks.push(async () => {
      const response = await fetch(PANDOC_WASM_URL, { cache: "no-cache" });
      if (!response.ok) return;
      await pandocCache.put(PANDOC_WASM_URL, response);
      for (const request of pandocWasms) await pandocCache.delete(request);
    });
  }
  let next = 0;
  let done = 0;
  setStatus({ downloading: { done, total: tasks.length } });
  const worker = async () => {
    // Stops when the copy pauses
    while (next < tasks.length && session === started) {
      await tasks[next++]();
      done += 1;
      setStatus({ downloading: { done, total: tasks.length } });
      if (done % 20 === 0) save();
    }
  };
  try {
    await Promise.all(
      Array.from(
        { length: Math.min(DOWNLOAD_CONCURRENCY, tasks.length) },
        worker,
      ),
    );
  } finally {
    save();
    if (changedAssets.size > 0) {
      await import("../typst/compiler")
        .then(({ forgetTypstFiles }) => forgetTypstFiles(changedAssets))
        .catch(console.error);
    }
  }
  // The device can't look at its files, so it shows notes and cards with their HTML (`web_path`)
  // only once it's here. An empty one means the rendered file instead.
  if (session === started) {
    const webRenders = files.filter(
      (file) =>
        file.path.endsWith(".html") &&
        file.size > 0 &&
        knownFiles[file.path] === file.mtime,
    );
    await call(
      "web_renders",
      webRenders.map((file) => file.path),
    );
  }

  // Through the service worker, which keeps them (`spares-typst-packages`), as it keeps the fonts
  // the compiler loads once it starts
  if (navigator.serviceWorker?.controller && session === started) {
    const specs = await listRenderPackages();
    await Promise.allSettled(specs.map((spec) => fetch(packageUrl(spec))));
    const { compileTypstInBackground } = await import("../typst/compiler");
    await compileTypstInBackground("").catch(console.error);
  }
}
