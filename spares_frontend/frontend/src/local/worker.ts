// Runs this device's copy of the collection (`spares_wasm`, built into ./pkg by
// `cargo xtask build-wasm`) for the page, which sends it calls with `postMessage` (see transport.ts).
// A Dedicated Worker, since only workers can reach the browser's storage (OPFS) synchronously, as
// SQLite needs.
import init, * as spares from "./pkg/spares_wasm.js";

/** A call to one of `spares_wasm`'s exports. */
export interface WorkerRequest {
  id: number;
  op: keyof typeof calls;
  args: unknown[];
}

export type WorkerResponse =
  | { id: number; result: unknown }
  | {
      id: number;
      error: {
        message: string;
        full_sync_required: boolean;
        not_enrolled: boolean;
        unreachable: boolean;
        busy: boolean;
      };
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
} as const;

const ready = init();

self.onmessage = async ({
  data: { id, op, args },
}: MessageEvent<WorkerRequest>) => {
  try {
    await ready;
    const result = await (
      calls[op] as (...args: unknown[]) => Promise<unknown>
    )(...args);
    // A response body is handed over rather than copied
    const body = (result as { body?: unknown } | null)?.body;
    self.postMessage({ id, result } satisfies WorkerResponse, {
      transfer: body instanceof Uint8Array ? [body.buffer as ArrayBuffer] : [],
    });
  } catch (e) {
    const error = e as {
      message?: unknown;
      full_sync_required?: unknown;
      not_enrolled?: unknown;
      unreachable?: unknown;
      busy?: unknown;
    } | null;
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
