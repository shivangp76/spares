// Runs this device's copy of the collection (`spares_wasm`, built into ./pkg by
// `cargo xtask build-wasm`) for the page, which sends it calls with `postMessage` (see transport.ts).
// A Dedicated Worker, since only workers can reach the browser's storage (OPFS) synchronously, as
// SQLite needs.
import init, * as spares from './pkg/spares_wasm.js';

/** A call to one of `spares_wasm`'s exports. */
export interface WorkerRequest {
  id: number;
  op: keyof typeof calls;
  args: unknown[];
}

export type WorkerResponse =
  | { id: number; result: unknown }
  | { id: number; error: { message: string; full_sync_required: boolean; not_enrolled: boolean; unreachable: boolean; busy: boolean } };

/** How much of the server's collection a full download has received, in bytes, sent while it runs */
export interface CollectionProgress {
  received: number;
  /** From `Content-Length`, unless the body turns out longer, as when it was compressed */
  total: number | null;
}

export type WorkerMessage = WorkerResponse | { progress: CollectionProgress | null };

/** How often a full download's progress is sent */
const PROGRESS_MS = 250;

// spares_wasm reads the whole collection of a full download at once (`HttpRemote::download_bytes`,
// with reqwest over `fetch`), which on a slow link takes minutes, so its body is counted here as
// it comes.
const fetchFromServer = self.fetch.bind(self);
self.fetch = async (input, init) => {
  const response = await fetchFromServer(input, init);
  const request = input instanceof Request ? input : null;
  const url = new URL(request?.url ?? String(input), self.location.href);
  if ((init?.method ?? request?.method ?? 'GET') !== 'GET' || !url.pathname.endsWith('/api/sync/full') || !response.ok || !response.body) {
    return response;
  }
  const length = Number(response.headers.get('Content-Length'));
  let total = length > 0 ? length : null;
  let received = 0;
  let sentAt = 0;
  const counted = response.body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      received += chunk.byteLength;
      if (total !== null && received > total) total = null;
      if (Date.now() - sentAt >= PROGRESS_MS) {
        sentAt = Date.now();
        self.postMessage({ progress: { received, total } } satisfies WorkerMessage);
      }
      controller.enqueue(chunk);
    },
    flush() {
      self.postMessage({ progress: null } satisfies WorkerMessage);
    },
  }));
  const replaced = new Response(counted, { status: response.status, statusText: response.statusText, headers: response.headers });
  // Which reqwest reads, and a new response has empty
  Object.defineProperty(replaced, 'url', { value: response.url });
  return replaced;
};

const calls = {
  open: spares.open,
  status: spares.status,
  handle: spares.handle,
  enroll: spares.enroll,
  sync: spares.sync,
  unenroll: spares.unenroll,
  forget: spares.forget,
  pending_notes: spares.pending_notes,
  render_assets: spares.render_assets,
  web_renders: spares.web_renders,
  media_file: spares.media_file,
  pandoc_config: spares.pandoc_config,
} as const;

const ready = init();

self.onmessage = async ({ data: { id, op, args } }: MessageEvent<WorkerRequest>) => {
  try {
    await ready;
    const result = await (calls[op] as (...args: unknown[]) => Promise<unknown>)(...args);
    // A response body is handed over rather than copied
    const body = (result as { body?: unknown } | null)?.body;
    self.postMessage({ id, result } satisfies WorkerResponse, { transfer: body instanceof Uint8Array ? [body.buffer as ArrayBuffer] : [] });
  } catch (e) {
    const error = e as { message?: unknown; full_sync_required?: unknown; not_enrolled?: unknown; unreachable?: unknown; busy?: unknown } | null;
    self.postMessage({
      id,
      error: {
        message: String(error?.message ?? e),
        full_sync_required: error?.full_sync_required === true,
        not_enrolled: error?.not_enrolled === true,
        unreachable: error?.unreachable === true,
        busy: error?.busy === true,
      },
    } satisfies WorkerResponse);
  }
};
