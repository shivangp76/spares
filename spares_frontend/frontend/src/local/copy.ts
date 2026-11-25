// This device's copy of the collection (Settings → This Device): setting it up, syncing it with the
// server, and downloading the files it shows offline.
//
// The copy is a sync replica of its own, like the laptop (see `spares_core::sync`): every page
// works on it, offline too, and a sync sends its changes to the server and brings the server's.
//
// **When it syncs.** When the app opens, when it is hidden or the connection comes back, once
// nothing changed for `IDLE_MS`, and with the Sync button, so a burst of changes, such as a
// review, is one sync. Syncing when hidden only happens if something changed or the last sync is
// `PULL_EVERY_MS` old. A change can still be undone after a sync, unless the sync changed the same
// notes or cards (see `spares_core::api::undo`).
//
// **Files.** The server renders notes, and the device keeps every rendered file and every file
// Typst cards read (`render_asset`) in Cache Storage, where the API client looks first. They are
// downloaded again when they change, after each sync, which lists only the server's files that
// changed since the last time (`listFiles`). So is pandoc.wasm, from the app's own
// build, which converts the Markdown cards of notes changed here (see pandoc.ts).
//
// **Logging in and out.** Everything the copy sends goes to its own server, with the session's CSRF
// token only when the credentials are for that server. On every change of credentials, the copy pauses: no
// syncs or downloads in the background, while the copy and its files stay. It starts again once
// logged in to its server.
import {
  copyKey,
  legacyCopy,
  legacyCopyKey,
  requireCopyAccount,
} from "./copyScope";
import {
  openCheckpoints,
  forgetCheckpoints,
  type RenderedFile,
  type FileListing,
} from "./copyCheckpoint";
import { useSyncExternalStore } from "react";
import {
  clearCaches,
  fileUrl,
  getCredentials,
  hostUrl,
  listRenderPackages,
  onCredentialsChange,
  renderAssetUrl,
  type SyncReport,
} from "../api/client";
import { reportServerReachable } from "../api/reachability";
import {
  call,
  canKeepCopy,
  COPY_FILES_CACHE,
  COPY_RENDER_ASSETS_CACHE,
  getLocalCopy,
  handleInCopyTab,
  inCopyTab,
  isCopyElsewhere,
  LocalError,
  onCollectionProgress,
  onCopyTab,
  onLocalWrite,
  openCopy,
  releaseCopy,
  resumeCopy,
  setLocalCopy,
  stopCopy,
  tellOtherTabs,
  usesLocalCopy,
} from "./transport";
import { COPY_PANDOC_CACHE, PANDOC_WASM_URL } from "./pandoc";
import type {
  CollectionProgress,
  WorkerMessage,
  WorkerRequest,
} from "./worker";

/** Syncs once nothing changed for this long */
const IDLE_MS = 30 * 1000;
/** Syncs even without changes of its own when the last sync is this old, to get the server's */
const PULL_EVERY_MS = 15 * 60 * 1000;
const CHECK_EVERY_MS = 10 * 1000;
/** How long a sync keeps trying while the server is busy, e.g. with a full upload's files */
const BUSY_RETRY_MS = 10 * 60 * 1000;
const BUSY_WAIT_MS = 15 * 1000;
/** Files downloaded at once */
const DOWNLOAD_CONCURRENCY = 4;
/** How often the bytes downloaded are shown while a file comes */
const PROGRESS_MS = 250;

// What has been downloaded in older builds. Migrated into IndexedDB on the first refresh.
const FILES_KEY = copyKey("spares_copy_files");
const ASSETS_KEY = copyKey("spares_copy_assets");
// The server's rendered files as last listed (`FileListing`)
const LISTING_KEY = copyKey("spares_copy_file_listing");
// Tells the service worker to keep spares_wasm and the Typst compiler offline too (see service-worker.js)
const SERVICE_WORKER_MARKER = copyKey("spares-local-copy");

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
  /** The server's collection, while a full download receives it */
  collection: CollectionProgress | null;
  /** Files being downloaded, and the bytes received of them so far */
  downloading: { done: number; total: number; bytes: number } | null;
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
  collection: null,
  downloading: null,
  pendingNotes: new Set(),
};

let status = OFF;
const listeners = new Set<() => void>();

function setStatus(update: Partial<LocalStatus>): void {
  status = { ...status, ...update };
  for (const listener of listeners) listener();
  tellOtherTabs("status", status);
}

function megabytes(bytes: number): string {
  return `${(bytes / 1e6).toFixed(1)} MB`;
}

/**
 * What the copy is downloading, as the app shows it, or `null`. Counted in bytes as well as files,
 * so that on a slow link it is seen to go on, through a large file such as pandoc.wasm too.
 */
export function describeDownload(local: LocalStatus): string | null {
  const { collection, downloading } = local;
  if (collection) {
    return `Downloading the collection, ${collection.total === null ? megabytes(collection.received) : `${megabytes(collection.received)} of ${megabytes(collection.total)}`}`;
  }
  if (downloading && downloading.total > 0) {
    return `Downloading files ${downloading.done}/${downloading.total}${downloading.bytes > 0 ? `, ${megabytes(downloading.bytes)}` : ""}`;
  }
  return null;
}

// A full download's progress, from this tab's worker
onCollectionProgress((collection) => setStatus({ collection }));

// Another tab, which has the copy, syncs it and downloads its files, so this one shows its status
handleInCopyTab("status", () => status);
onCopyTab("status", (data) => {
  if (getLocalCopy() === null) {
    // It stopped keeping the copy
    if (status.active) {
      clearCaches();
      status = OFF;
      for (const listener of listeners) listener();
    }
    return;
  }
  setStatus(otherTabStatus(data as LocalStatus));
});

/** The status of another tab's copy, as this one shows it: whether this tab uses it is its own */
function otherTabStatus(other: LocalStatus): Partial<LocalStatus> {
  const shown: Partial<LocalStatus> = { ...other };
  delete shown.active;
  return shown;
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
export function isPendingRender(noteId: number): boolean {
  return status.pendingNotes.has(noteId);
}

/** `isPendingRender`, kept up to date. */
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

/** The session's CSRF token, or `null` when logged out. */
function sessionToken(): string | null {
  try {
    return getCredentials().csrfToken;
  } catch {
    return null;
  }
}

/** The host the copy syncs with, and the session's CSRF token, throwing unless logged in. */
function copyCredentials(): { serverUrl: string; csrfToken: string } {
  if (!getLocalCopy()) throw new Error("This device doesn't keep a copy.");
  const csrfToken = sessionToken();
  if (csrfToken === null) throw new Error("Log in to sync this device's copy.");
  return { serverUrl: hostUrl(), csrfToken };
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
export async function syncLocalCopy(
  mode: "normal" | "full_download" | "full_upload" = "normal",
): Promise<SyncReport> {
  const report = await inCopyTab<SyncReport>("sync", mode);
  // The other tab cleared its own
  if (isCopyElsewhere() && report.mode !== null) clearCaches();
  return report;
}

/**
 * Runs `action`, a request only the server answers (`needs_server`), for a device that keeps a
 * copy of the collection: syncs first, so the server has the copy's changes, and afterwards, so
 * the copy gets what `action` changed. `local` says whether it did; a failure of the second sync
 * is in `syncError`. The server's events aren't synced, so the copy can't undo what `action` did.
 */
export async function runOnServer<T>(
  action: () => Promise<T>,
): Promise<{ result: T; local: boolean; syncError: string | null }> {
  const local = usesLocalCopy();
  if (local) await syncLocalCopy();
  const result = await action();
  let syncError: string | null = null;
  if (local) {
    try {
      await syncLocalCopy();
    } catch (e) {
      syncError = e instanceof Error ? e.message : String(e);
    }
  }
  return { result, local, syncError };
}

handleInCopyTab("sync", syncHere);

/** `syncLocalCopy` in the tab with the copy */
function syncHere(
  mode: "normal" | "full_download" | "full_upload",
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
      refreshFilesHere().catch((e) => console.error("Downloading files", e));
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
      // Also when the download stopped half way
      setStatus({ syncing: false, collection: null });
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
  server: { serverUrl: string; csrfToken: string },
  mode: "normal" | "full_download" | "full_upload",
): Promise<SyncReport> {
  const started = Date.now();
  for (;;) {
    try {
      return await call<SyncReport>(
        "sync",
        server.serverUrl,
        server.csrfToken,
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
    sessionToken() === null
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

/** Opens the copy if this device keeps one and is logged in, syncs it, and keeps syncing it in the background. */
export async function startLocalCopy(): Promise<void> {
  if (status.active || sessionToken() === null || !usesLocalCopy()) return;
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
    // Whether it is syncing or downloading
    if (isCopyElsewhere()) {
      setStatus(otherTabStatus(await inCopyTab<LocalStatus>("status")));
    }
  } catch (e) {
    // E.g. the tab that has it open isn't answering
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
  if (event.persisted) releaseCopy();
});
window.addEventListener("pageshow", (event) => {
  if (!event.persisted) return;
  resumeCopy();
  if (!status.active) return;
  openLocalCopy().then((opened) => {
    if (opened && worthSyncing(Date.now())) syncInBackground();
  }, console.error);
});

// Bumped when the copy pauses, which stops a download that is running
let session = 0;

onCredentialsChange(() => {
  session += 1;
  releaseCopy();
  setStatus(OFF);
});

// Setting up and removing the copy

/** Restore an unlabelled copy only after its saved collection and host account both match. */
export async function recoverLegacyCopy(): Promise<void> {
  const account = requireCopyAccount();
  const csrf = getCredentials().csrfToken;
  if (!navigator.locks)
    throw new Error("Recovery needs a browser with Web Locks support.");
  await navigator.locks.request("spares-copy-setup:" + account, () =>
    navigator.locks.request(
      "spares-db",
      { ifAvailable: true },
      async (held) => {
        if (!held)
          throw new Error(
            "Close other tabs using the older copy and try again.",
          );
        const old = legacyCopy();
        if (!old || old.collectionId || getLocalCopy())
          throw new Error("The offline copy changed. Reload Settings.");
        const message =
          "This account’s enrollment and collection could not be verified. The older copy has been preserved.";
        const verifier = new Worker(new URL("./worker.ts", import.meta.url), {
          type: "module",
        });
        try {
          const matches = await new Promise<boolean>((resolve, reject) => {
            verifier.onerror = () => reject(new Error(message));
            verifier.onmessage = ({ data }: MessageEvent<WorkerMessage>) => {
              if ("error" in data)
                reject(
                  new Error(
                    data.error.message + " The older copy has been preserved.",
                  ),
                );
              else if ("result" in data) resolve(data.result === true);
            };
            verifier.postMessage({
              id: 1,
              op: "legacy_matches",
              args: [account, hostUrl(), csrf, old.replica],
            } satisfies WorkerRequest);
          });
          requireCopyAccount();
          if (
            !matches ||
            getCredentials().csrfToken !== csrf ||
            getLocalCopy() ||
            legacyCopy()?.collectionId
          )
            throw new Error(message);
          localStorage.setItem(
            legacyCopyKey,
            JSON.stringify({ ...old, collectionId: account }),
          );
        } finally {
          verifier.terminate();
        }
      },
    ),
  );
  window.location.reload();
}

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
  const account = requireCopyAccount();
  const setup = async () => {
    if (getLocalCopy()?.ready) {
      await startLocalCopy();
      return;
    }
    await setUpCopy(name, onStep);
  };
  if (navigator.locks)
    await navigator.locks.request("spares-copy-setup:" + account, setup);
  else await setup();
}

async function setUpCopy(name: string, onStep: (step: SetupStep) => void) {
  if (!(await canKeepCopy()))
    throw new Error(
      "This browser has no storage for a copy, as in a private window.",
    );
  const csrfToken = sessionToken();
  if (csrfToken === null) throw new Error("Log in first.");
  const serverUrl = hostUrl();
  onStep("Registering this device");
  const state = (await openCopy()) as CopyState;
  // Already enrolled when an earlier attempt stopped after it
  const replica =
    state.replica ?? (await call<number>("enroll", serverUrl, csrfToken, name));
  const collectionId = requireCopyAccount();
  setLocalCopy({ replica, collectionId, ready: false });
  onStep("Downloading the collection");
  try {
    // Missing browser metadata can coexist with a valid account database containing offline edits.
    if (state.last_sync_at === null)
      await call<SyncReport>("sync", serverUrl, csrfToken, "full_download");
  } finally {
    setStatus({ collection: null });
  }
  setLocalCopy({ replica, collectionId, ready: true });
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
  await forgetCheckpoints();
  localStorage.removeItem(FILES_KEY);
  localStorage.removeItem(ASSETS_KEY);
  localStorage.removeItem(LISTING_KEY);
  for (const name of [
    SERVICE_WORKER_MARKER,
    COPY_FILES_CACHE,
    COPY_RENDER_ASSETS_CACHE,
  ]) {
    await globalThis.caches?.delete(name).catch(console.error);
  }
  // The versioned compiler is public and identical for every account on this host.
  const remaining = await globalThis.caches?.keys();
  if (
    remaining &&
    !remaining.some(
      (key) =>
        key === "spares-local-copy" || key.startsWith("spares-local-copy:"),
    )
  )
    await caches.delete(COPY_PANDOC_CACHE).catch(console.error);
  clearCaches();
  status = OFF;
  for (const listener of listeners) listener();
  tellOtherTabs("status", status);
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
  return inCopyTab<string | null>("disable", discardChanges);
}

handleInCopyTab("disable", disableHere);

/** `disableLocalCopy` in the tab with the copy */
async function disableHere(discardChanges: boolean): Promise<string | null> {
  if (!discardChanges && !status.notEnrolled) await syncLocalCopy();
  const replica = getLocalCopy()?.replica ?? status.replica;
  let warning: string | null = null;
  if (!status.notEnrolled) {
    try {
      const { serverUrl, csrfToken } = copyCredentials();
      // Which deletes the copy too
      await call("unenroll", serverUrl, csrfToken);
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

/** The server's rendered files, from the listing here and what changed on the server since. */
async function listFiles(
  serverUrl: string,
  headers: Record<string, string>,
  listing: FileListing,
): Promise<{ files: RenderedFile[]; changed: Record<string, string> }> {
  const res = await fetch(`${serverUrl}/api/sync/files/changed`, {
    method: "POST",
    headers: { ...headers, "Content-Type": "application/json" },
    body: JSON.stringify({ buckets: listing.buckets }),
  });
  if (!res.ok)
    throw new Error(`Listing the server's files failed: ${res.status}`);
  const { buckets, files } = (await res.json()) as {
    buckets: Record<string, string>;
    files: RenderedFile[];
  };
  // Those listed again, and those that have no files left
  const changed: Record<string, string> = {};
  for (const [bucket, digest] of Object.entries(buckets))
    if (listing.buckets[bucket] !== digest) changed[bucket] = digest;
  for (const bucket of Object.keys(listing.buckets))
    if (!(bucket in buckets)) changed[bucket] = "";
  const unchanged = Object.values(listing.files).filter(
    (file) => !(file.bucket in changed) && file.bucket in buckets,
  );
  return { files: unchanged.concat(files), changed };
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
  return inCopyTab("refreshFiles");
}

handleInCopyTab("refreshFiles", refreshFilesHere);

/** `refreshFiles` in the tab with the copy */
function refreshFilesHere(): Promise<void> {
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
  const { serverUrl, csrfToken } = copyCredentials();
  const headers = { "X-CSRF-Token": csrfToken };
  const checkpoint = await openCheckpoints();
  let success = false;
  try {
    await refreshFromCheckpoint(checkpoint, started, serverUrl, headers);
    success = session === started;
  } finally {
    checkpoint.close(success);
  }
}

async function refreshFromCheckpoint(
  checkpoint: Awaited<ReturnType<typeof openCheckpoints>>,
  started: number,
  serverUrl: string,
  headers: Record<string, string>,
) {
  // Older copies kept full maps in localStorage. Re-list once and reuse their completed downloads.
  const migrating = [FILES_KEY, ASSETS_KEY, LISTING_KEY].some(
    (key) => localStorage.getItem(key) !== null,
  );
  const knownFiles: Record<string, string | number> = migrating
    ? readMap(FILES_KEY)
    : Object.create(null);
  const knownAssets = migrating
    ? { ...readMap(ASSETS_KEY), ...checkpoint.assets }
    : checkpoint.assets;
  for (const file of Object.values(checkpoint.listing.files))
    knownFiles[file.path] = file.mtime;
  const priorFiles = checkpoint.listing.files;
  const { files, changed } = await listFiles(
    serverUrl,
    headers,
    checkpoint.listing,
  );
  const assets = await call<{ path: string; hash: string }[]>("render_assets");

  const [filesCache, assetsCache, pandocCache] = await Promise.all([
    caches.open(COPY_FILES_CACHE),
    caches.open(COPY_RENDER_ASSETS_CACHE),
    caches.open(COPY_PANDOC_CACHE),
  ]);
  const current = new Set(files.map((file) => file.path));
  for (const path of Object.keys(knownFiles)) {
    if (current.has(path)) continue;
    await filesCache.delete(fileUrl(path));
    delete knownFiles[path];
    checkpoint.removeFile(path);
  }
  // Also those the device no longer needs, such as images of cards' image occlusions, which it makes itself
  const currentAssets = new Set(assets.map((asset) => asset.path));
  // For the Typst compiler, which otherwise keeps what it read of them while the page is open
  const changedAssets = new Set<string>();
  for (const path of Object.keys(knownAssets)) {
    if (currentAssets.has(path)) continue;
    await assetsCache.delete(renderAssetUrl(path));
    delete knownAssets[path];
    checkpoint.removeAsset(path);
    changedAssets.add(path);
  }
  const missing = new Map<string, number>();
  for (const file of files) {
    const old = priorFiles[file.path];
    if (old && old.size !== file.size) delete knownFiles[file.path];
    if (knownFiles[file.path] !== file.mtime)
      missing.set(file.bucket, (missing.get(file.bucket) ?? 0) + 1);
    else if (migrating || file.bucket in changed) checkpoint.file(file);
  }
  if (migrating) {
    for (const [path, hash] of Object.entries(knownAssets))
      if (typeof hash === "string") checkpoint.asset(path, hash);
  }
  for (const [bucket, digest] of Object.entries(changed)) {
    // Invalidate the old digest even if the refresh is interrupted and the server reverts.
    checkpoint.bucket(bucket, missing.has(bucket) ? "" : digest);
  }
  const save = () =>
    session === started ? checkpoint.save() : Promise.resolve();
  await save();
  if (migrating && session === started) {
    for (const key of [FILES_KEY, ASSETS_KEY, LISTING_KEY])
      localStorage.removeItem(key);
  }

  let next = 0;
  let done = 0;
  let bytes = 0;
  const showProgress = () =>
    setStatus({ downloading: { done, total: tasks.length, bytes } });
  // `response` with its body counted in `bytes` as `cache.put` reads it
  const counted = (response: Response): Response => {
    if (!response.body) return response;
    let shownAt = 0;
    const body = response.body.pipeThrough(
      new TransformStream<Uint8Array, Uint8Array>({
        transform(chunk, controller) {
          bytes += chunk.byteLength;
          if (Date.now() - shownAt >= PROGRESS_MS) {
            shownAt = Date.now();
            showProgress();
          }
          controller.enqueue(chunk);
        },
      }),
    );
    // Which no longer describe the body, which the browser has decoded
    const kept = new Headers(response.headers);
    kept.delete("Content-Length");
    kept.delete("Content-Encoding");
    return new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: kept,
    });
  };

  const tasks: (() => Promise<void>)[] = [];
  // The build's, which names its version. An earlier build's goes once this one's is here. First,
  // since it is the largest by far (about 11 MB), so the other files come meanwhile.
  const pandocWasms = await pandocCache.keys();
  if (!pandocWasms.some((request) => request.url === PANDOC_WASM_URL)) {
    tasks.push(async () => {
      const response = await fetch(PANDOC_WASM_URL, { cache: "no-cache" });
      if (!response.ok) return;
      await pandocCache.put(PANDOC_WASM_URL, counted(response));
      for (const request of pandocWasms) await pandocCache.delete(request);
    });
  }
  tasks.push(
    ...files
      .filter((file) => knownFiles[file.path] !== file.mtime)
      .map((file) => async () => {
        const url = fileUrl(file.path);
        const response = await fetch(url, { headers, cache: "no-cache" });
        // A file that went away meanwhile is left for the next time
        if (!response.ok) return;
        await filesCache.put(url, counted(response));
        knownFiles[file.path] = file.mtime;
        checkpoint.file(file);
        const left = (missing.get(file.bucket) ?? 1) - 1;
        missing.set(file.bucket, left);
        if (left === 0 && changed[file.bucket])
          checkpoint.bucket(file.bucket, changed[file.bucket]);
      }),
    ...assets
      .filter((asset) => knownAssets[asset.path] !== asset.hash)
      .map((asset) => async () => {
        const url = renderAssetUrl(asset.path);
        const response = await fetch(url, { headers, cache: "no-cache" });
        if (!response.ok && response.status !== 404) return;
        // A 404 is fetched again at the next refresh: the server may only be missing the file for
        // now, as when `spares sync cloud` has sent the database and not yet the files. Meanwhile
        // the version the device has, if any, is kept. Otherwise the 404 is, so that offline the
        // file isn't asked for.
        if (response.ok) {
          await assetsCache.put(url, counted(response));
          knownAssets[asset.path] = asset.hash;
          checkpoint.asset(asset.path, asset.hash);
          changedAssets.add(asset.path);
        } else if (!(await assetsCache.match(url, { ignoreVary: true }))) {
          await assetsCache.put(url, response);
        }
      }),
  );
  showProgress();
  const worker = async () => {
    // Stops when the copy pauses
    while (next < tasks.length && session === started) {
      await tasks[next++]();
      done += 1;
      showProgress();
      if (done % 100 === 0) await save();
    }
  };
  try {
    const results = await Promise.allSettled(
      Array.from(
        { length: Math.min(DOWNLOAD_CONCURRENCY, tasks.length) },
        worker,
      ),
    );
    for (const result of results)
      if (result.status === "rejected") throw result.reason;
  } finally {
    await save();
    if (changedAssets.size > 0) {
      await import("../typst/compiler")
        .then(({ forgetTypstFiles }) => forgetTypstFiles(changedAssets))
        .catch(console.error);
    }
  }
  // The device can't look at its files, so it shows notes and cards with their HTML (`web_path`)
  // only once it's here. Without one, or with an empty one, which servers wrote before, the
  // rendered file.
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
