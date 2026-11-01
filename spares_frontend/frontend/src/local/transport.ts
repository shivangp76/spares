// The page's side of this device's copy of the collection: whether there is one, and calls to the
// worker that runs it (worker.ts). The API client sends requests here instead of to the server
// when the device has a copy (see `apiSend` in src/api/client.ts), and copy.ts sets it up and syncs it.
//
// The browser's storage is opened by one tab at a time, since SQLite's files there have no locks
// between tabs. The tab that opens it holds the Web Lock `spares-db` until it closes. Other tabs
// send their requests to it over the `BroadcastChannel` `spares-copy`, and take over once the
// lock is free, by reloading. It answers each request at once with an `ack`, so a tab whose
// requests go unanswered, as when the browser froze the other tab in the background, says so
// rather than waiting, and it drops a request sent too long ago (`STALE_MS`). A page kept in the
// back-forward cache lets go of the lock and the channel (`releaseCopy`), and takes them again when
// it is shown.
//
// Some of what copy.ts does, such as syncing and downloading files, only the tab with the copy
// does: other tabs ask it to (`inCopyTab`), and it tells them its status (`tellOtherTabs`).
import type { WorkerRequest, WorkerResponse } from './worker';

const COPY_KEY = 'spares_local_copy';
const LOCK_NAME = 'spares-db';

/** Where the copy keeps the rendered files and render assets it downloads (see copy.ts), by URL on its server. */
export const COPY_FILES_CACHE = 'spares-copy-files';
export const COPY_RENDER_ASSETS_CACHE = 'spares-copy-render-assets';

/** The copy this device keeps, as set up in Settings. */
export interface LocalCopy {
  /** The server it syncs with. Logged in to another, the device is a thin client again. */
  serverUrl: string;
  replica: number;
  /** Whether its first full download has finished. Until then, requests go to the server. */
  ready: boolean;
}

export function getLocalCopy(): LocalCopy | null {
  try {
    const raw = localStorage.getItem(COPY_KEY);
    return raw ? (JSON.parse(raw) as LocalCopy) : null;
  } catch {
    return null;
  }
}

export function setLocalCopy(copy: LocalCopy | null): void {
  if (copy) localStorage.setItem(COPY_KEY, JSON.stringify(copy));
  else localStorage.removeItem(COPY_KEY);
}

/** Whether requests to `serverUrl` are answered by this device's copy. */
export function usesLocalCopy(serverUrl: string): boolean {
  const copy = getLocalCopy();
  return copy !== null && copy.ready && copy.serverUrl === serverUrl;
}

let storageProbe: Promise<boolean> | null = null;

/** Whether this browser can keep a copy: it needs workers, and storage only the origin can see (OPFS), which private windows may not have, or have and refuse, as WebKit's do. */
export function canKeepCopy(): Promise<boolean> {
  if (typeof Worker === 'undefined' || typeof navigator.storage?.getDirectory !== 'function') return Promise.resolve(false);
  storageProbe ??= navigator.storage.getDirectory().then(() => true, () => false);
  return storageProbe;
}

/** What a `LocalError` says about the sync that failed, from spares_wasm's error (see its docs). */
export interface LocalErrorFlags {
  /** The next sync must download or upload the whole collection */
  fullSyncRequired?: boolean;
  /** The server no longer knows this device */
  notEnrolled?: boolean;
  /** The server couldn't be reached */
  unreachable?: boolean;
  /** The server is busy with another change, such as a full upload's files, so trying again shortly may work */
  busy?: boolean;
}

/** An error of the copy. */
export class LocalError extends Error {
  fullSyncRequired: boolean;
  notEnrolled: boolean;
  unreachable: boolean;
  busy: boolean;

  constructor(message: string, flags: LocalErrorFlags = {}) {
    super(message);
    this.name = 'LocalError';
    this.fullSyncRequired = flags.fullSyncRequired ?? false;
    this.notEnrolled = flags.notEnrolled ?? false;
    this.unreachable = flags.unreachable ?? false;
    this.busy = flags.busy ?? false;
  }
}

// Which tab has the copy open

/** `elsewhere` while another tab has the copy open, which this one sends its requests to. */
type CopyOwner = 'unknown' | 'this-tab' | 'elsewhere';

let owner: CopyOwner = 'unknown';

/** Whether another tab has the copy open, which runs what this one asks of it. */
export function isCopyElsewhere(): boolean {
  return owner === 'elsewhere';
}

let lock: Promise<boolean> | null = null;
// Ends the lock request `takeLock` made, held or waiting
let lockRequest: AbortController | null = null;

/** Takes the lock for this tab, for as long as it is open. False if another tab has it. */
function takeLock(): Promise<boolean> {
  // Without Web Locks (before Safari 15.4), tabs aren't kept apart
  if (!navigator.locks) return Promise.resolve(true);
  lock ??= new Promise<boolean>(resolve => {
    const request = new AbortController();
    lockRequest = request;
    navigator.locks.request(LOCK_NAME, { ifAvailable: true }, held => {
      resolve(held !== null);
      // Held until the tab closes, or `releaseCopy`
      return held ? new Promise<void>(release => request.signal.addEventListener('abort', () => release())) : undefined;
    }).catch(() => resolve(true));
  }).then(held => {
    owner = held ? 'this-tab' : 'elsewhere';
    openChannel();
    if (!held) {
      // Starts again as the owner once the other tab closes
      navigator.locks.request(LOCK_NAME, { signal: lockRequest?.signal }, () => {
        window.location.reload();
        return new Promise<void>(() => {});
      }).catch(e => { if (!(e instanceof DOMException && e.name === 'AbortError')) console.error(e); });
    }
    return held;
  });
  return lock;
}

// Other tabs

const CHANNEL_NAME = 'spares-copy';
/** How long a request waits for the tab with the copy to say it has it */
const ACK_MS = 5 * 1000;
/**
 * How old a request the tab with the copy still takes, by when it was sent. Shorter than `ACK_MS`,
 * so the tab that sent it hears back before it stops waiting. A tab the browser paused gets the
 * requests that came meanwhile once it runs again, and drops these, which the tabs that sent them
 * have given up on: a change the user was told failed would otherwise be made after all.
 */
const STALE_MS = ACK_MS - 1000;
const NOT_ANSWERING = "The tab that has this device's copy isn't answering, as when the browser paused it in the background. Switch to it, or close it to use this one.";

/** A request from another tab, and what the tab with the copy sends back and tells every tab */
type ChannelMessage =
  | { kind: 'call'; id: string; sentAt: number; op: WorkerRequest['op']; args: unknown[] }
  | { kind: 'command'; id: string; sentAt: number; name: string; args: unknown[] }
  | { kind: 'ack'; id: string }
  | { kind: 'reply'; id: string; result: unknown }
  | { kind: 'reply'; id: string; error: { message: string; flags: LocalErrorFlags } }
  | { kind: 'event'; name: string; data: unknown };

let channel: BroadcastChannel | null = null;
const tabId = Math.random().toString(36).slice(2);
let nextRequest = 0;
const requests = new Map<string, { resolve: (result: unknown) => void; reject: (error: Error) => void; timer?: ReturnType<typeof setTimeout> }>();
const commands = new Map<string, (...args: never[]) => unknown>();
const eventListeners = new Map<string, Set<(data: unknown) => void>>();

function openChannel(): void {
  if (channel || typeof BroadcastChannel === 'undefined') return;
  channel = new BroadcastChannel(CHANNEL_NAME);
  channel.onmessage = ({ data }: MessageEvent<ChannelMessage>) => {
    if (owner === 'this-tab' && (data.kind === 'call' || data.kind === 'command')) {
      answer(data).catch(console.error);
    } else if (owner === 'elsewhere' && (data.kind === 'ack' || data.kind === 'reply')) {
      const request = requests.get(data.id);
      if (!request) return;
      clearTimeout(request.timer);
      if (data.kind === 'ack') return;
      requests.delete(data.id);
      if ('error' in data) request.reject(new LocalError(data.error.message, data.error.flags));
      else request.resolve(data.result);
    } else if (owner === 'elsewhere' && data.kind === 'event') {
      for (const listener of eventListeners.get(data.name) ?? []) listener(data.data);
    }
  };
}

function closeChannel(): void {
  channel?.close();
  channel = null;
  for (const request of requests.values()) {
    clearTimeout(request.timer);
    request.reject(new LocalError('This page was left, so it has closed the copy.'));
  }
  requests.clear();
}

/** Runs another tab's request here, in the tab with the copy. */
async function answer(message: Extract<ChannelMessage, { kind: 'call' | 'command' }>): Promise<void> {
  if (Date.now() - message.sentAt > STALE_MS) return;
  channel?.postMessage({ kind: 'ack', id: message.id } satisfies ChannelMessage);
  let reply: ChannelMessage;
  try {
    let result: unknown;
    if (message.kind === 'command') {
      const command = commands.get(message.name);
      if (!command) throw new LocalError(`The tab that has this device's copy can't ${message.name}: reload it.`);
      result = await command(...(message.args as never[]));
    } else {
      await openCopy();
      // The copy's status, since it is already open
      result = await post(message.op === 'open' ? 'status' : message.op, message.args);
      const [method] = message.args;
      if (message.op === 'handle' && method !== 'GET') changed();
    }
    reply = { kind: 'reply', id: message.id, result };
  } catch (e) {
    const local = e instanceof LocalError ? e : null;
    reply = {
      kind: 'reply',
      id: message.id,
      error: {
        message: e instanceof Error ? e.message : String(e),
        flags: { fullSyncRequired: local?.fullSyncRequired, notEnrolled: local?.notEnrolled, unreachable: local?.unreachable, busy: local?.busy },
      },
    };
  }
  channel?.postMessage(reply);
}

/** Sends a request to the tab with the copy, and waits for its answer. */
function askCopyTab<T>(message: { kind: 'call'; op: WorkerRequest['op']; args: unknown[] } | { kind: 'command'; name: string; args: unknown[] }): Promise<T> {
  const id = `${tabId}-${nextRequest++}`;
  return new Promise<T>((resolve, reject) => {
    if (!channel) {
      reject(new LocalError('spares is open in another tab, which has this device\'s copy. Close it to use this one.'));
      return;
    }
    const timer = setTimeout(() => {
      requests.delete(id);
      reject(new LocalError(NOT_ANSWERING));
    }, ACK_MS);
    requests.set(id, { resolve: resolve as (result: unknown) => void, reject, timer });
    channel.postMessage({ ...message, id, sentAt: Date.now() } satisfies ChannelMessage);
  });
}

/** Lets other tabs run `command` here, when this tab has the copy (see `inCopyTab`). */
export function handleInCopyTab<A extends unknown[], T>(name: string, command: (...args: A) => T | Promise<T>): void {
  commands.set(name, command as (...args: never[]) => unknown);
}

/** Runs the command `name` (see `handleInCopyTab`) in the tab with the copy: this one, or another. */
export async function inCopyTab<T>(name: string, ...args: unknown[]): Promise<T> {
  await openCopy();
  if (owner === 'elsewhere') return askCopyTab<T>({ kind: 'command', name, args });
  const command = commands.get(name);
  if (!command) throw new Error(`No command ${name}`);
  return await command(...(args as never[])) as T;
}

/** Tells the other tabs `data`, which their `onCopyTab(name)` listeners get. Only the tab with the copy does. */
export function tellOtherTabs(name: string, data: unknown): void {
  if (owner === 'this-tab') channel?.postMessage({ kind: 'event', name, data } satisfies ChannelMessage);
}

/** Calls `listener` with what the tab with the copy tells the other tabs (see `tellOtherTabs`), while this isn't it. */
export function onCopyTab(name: string, listener: (data: unknown) => void): () => void {
  let listeners = eventListeners.get(name);
  if (!listeners) eventListeners.set(name, listeners = new Set());
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// The worker

let worker: Worker | null = null;
let nextId = 0;
const waiting = new Map<number, { resolve: (result: unknown) => void; reject: (error: Error) => void }>();

function startWorker(): Worker {
  const started = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
  started.onmessage = ({ data }: MessageEvent<WorkerResponse>) => {
    const call = waiting.get(data.id);
    if (!call) return;
    waiting.delete(data.id);
    if ('error' in data) {
      const { message, full_sync_required, not_enrolled, unreachable, busy } = data.error;
      call.reject(new LocalError(message, { fullSyncRequired: full_sync_required, notEnrolled: not_enrolled, unreachable, busy }));
    } else call.resolve(data.result);
  };
  started.onerror = event => {
    // E.g. the module failed to load. Every call waiting fails, and the next starts a new worker.
    event.preventDefault();
    for (const call of waiting.values()) call.reject(new LocalError(`This device's copy stopped: ${event.message || 'its worker failed to load'}`));
    waiting.clear();
    started.terminate();
    if (worker === started) { worker = null; opened = null; }
  };
  return started;
}

/** Sends `op` to the worker, or to the tab with the copy while that is another. */
function post<T>(op: WorkerRequest['op'], args: unknown[], transfer: Transferable[] = []): Promise<T> {
  if (owner === 'elsewhere') return askCopyTab<T>({ kind: 'call', op, args });
  worker ??= startWorker();
  const id = nextId++;
  return new Promise<T>((resolve, reject) => {
    waiting.set(id, { resolve: resolve as (result: unknown) => void, reject });
    worker!.postMessage({ id, op, args } satisfies WorkerRequest, transfer);
  });
}

let opened: Promise<unknown> | null = null;
// Between `releaseCopy` and `resumeCopy`, so nothing that was running opens the copy again
let released = false;

/** How long `openCopy` keeps trying while the browser's storage is still held by a page that is going away */
const OPEN_RETRY_MS = 10 * 1000;

/**
 * Whether opening failed because another page still holds the browser's storage: one this page
 * replaced, whose worker hasn't stopped yet. The lock is free by then, so it isn't another tab.
 */
function storageStillHeld(e: unknown): boolean {
  return e instanceof Error && e.message.includes('InvalidStateError');
}

/** Opens the copy in this tab, creating an empty one the first time, and returns its status (see `call('status')`). */
export function openCopy(): Promise<unknown> {
  if (released) return Promise.reject(new LocalError('This page was left, so it has closed the copy.'));
  opened ??= (async () => {
    // Through the tab that has it
    if (!(await takeLock())) return post('open', []);
    const started = Date.now();
    for (let wait = 100; ; wait = Math.min(wait * 2, 2000)) {
      try {
        return await post('open', []);
      } catch (e) {
        if (!storageStillHeld(e) || Date.now() - started > OPEN_RETRY_MS) throw e;
      }
      // In a new worker, since the failed one may keep what it set up
      worker?.terminate();
      worker = null;
      await new Promise(resolve => setTimeout(resolve, wait));
    }
  })();
  opened.catch(() => { opened = null; });
  return opened;
}

/** Calls `op` of spares_wasm, once the copy is open. */
export async function call<T>(op: Exclude<WorkerRequest['op'], 'open'>, ...args: unknown[]): Promise<T> {
  await openCopy();
  return post<T>(op, args);
}

/** Stops the worker, e.g. after the copy was deleted, so the next call opens it afresh. */
export function stopCopy(): void {
  worker?.terminate();
  worker = null;
  opened = null;
  for (const call of waiting.values()) call.reject(new LocalError('This device\'s copy was closed.'));
  waiting.clear();
}

/**
 * Closes the copy and frees the lock until the next call opens it again. For a page that goes
 * into the back-forward cache: its worker would keep the browser's storage, which the next page
 * needs, and Safari frees the lock anyway.
 */
export function releaseCopy(): void {
  released = true;
  stopCopy();
  lockRequest?.abort();
  lockRequest = null;
  lock = null;
  closeChannel();
  owner = 'unknown';
}

/** Lets the next call open the copy again after `releaseCopy`, once the page is shown again. */
export function resumeCopy(): void {
  released = false;
}

// Requests

const writeListeners = new Set<() => void>();

/** Calls `listener` after every request that may change the copy, from this tab or another. */
export function onLocalWrite(listener: () => void): () => void {
  writeListeners.add(listener);
  return () => writeListeners.delete(listener);
}

/** After a request that may have changed the copy: here, and in the other tabs. */
function changed(): void {
  for (const listener of writeListeners) listener();
  tellOtherTabs('write', null);
}

onCopyTab('write', () => {
  for (const listener of writeListeners) listener();
});

// Statuses whose responses have no body
const NULL_BODY_STATUSES = [101, 204, 205, 304];

/** `fetch` of an API path (`/api/...`), answered by this device's copy. Requests it can't answer get `503 {needs_server: true}`. */
export async function localFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const method = init.method ?? 'GET';
  const headers = new Headers(init.headers);
  const encoded = init.body == null ? null : new Response(init.body);
  // A `FormData` body's multipart type, with its boundary, as `fetch` would send it
  const bodyType = encoded?.headers.get('Content-Type');
  if (bodyType && !headers.has('Content-Type')) headers.set('Content-Type', bodyType);
  const body = encoded ? new Uint8Array(await encoded.arrayBuffer()) : undefined;
  await openCopy();
  const res = await post<{ status: number; headers: [string, string][]; body: Uint8Array }>(
    'handle', [method, path, [...headers.entries()], body], body ? [body.buffer] : [],
  );
  if (method !== 'GET') {
    // The tab with the copy tells the others
    if (owner === 'elsewhere') for (const listener of writeListeners) listener();
    else changed();
  }
  return new Response(NULL_BODY_STATUSES.includes(res.status) ? null : res.body as Uint8Array<ArrayBuffer>, { status: res.status, headers: res.headers });
}

/** An image, clip or image occlusion file this device has, by the path notes name it by (see `spares_wasm::media_file`). */
export async function deviceMediaFile(path: string): Promise<Uint8Array | null> {
  return (await call<Uint8Array | undefined>('media_file', path)) ?? null;
}

/** Whether `res` is the copy's answer to a request only the server can answer. Reads a copy of the body. */
export async function needsServer(res: Response): Promise<boolean> {
  if (res.status !== 503) return false;
  try {
    return (await res.clone().json() as { needs_server?: unknown }).needs_server === true;
  } catch {
    return false;
  }
}
