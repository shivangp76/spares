// The Markdown cards of notes changed on this device, converted as the server renders them, with
// pandoc's WebAssembly build (pandocWasm.ts), until the server renders them after the next sync.
// `DeviceSource` in CardRenderer shows a card with `marked` first, and swaps in pandoc's HTML when
// it comes. Until the copy has downloaded pandoc.wasm (copy.ts), and if pandoc fails, the `marked`
// HTML stays.
//
// pandoc runs in a worker of its own (pandoc.worker.ts), started when a card is first asked for
// and stopped after `IDLE_MS` without one, which frees its memory. Browsers without a copy, cards
// the server has rendered, and Typst and LaTeX cards never start it.
import { useEffect, useState } from "react";
import type { PandocRequest, PandocResponse } from "./pandoc.worker";
import { call } from "./transport";

/** pandoc.wasm of the server's version (see pandoc-wasm.ts), which the copy keeps in `COPY_PANDOC_CACHE` */
export const PANDOC_WASM_URL = new URL(__PANDOC_WASM__, location.origin).href;
export const COPY_PANDOC_CACHE = "spares-copy-pandoc";

const IDLE_MS = 60 * 1000;

/** spares_wasm's `pandoc_config()` */
interface PandocConfig {
  version: string | null;
  web_defaults: string | null;
}

let worker: Worker | null = null;
// Read again for each worker, so it follows the server's web defaults file
let config: Promise<PandocConfig> | null = null;
let stopTimer: ReturnType<typeof setTimeout> | undefined;
let nextId = 0;
const waiting = new Map<
  number,
  { resolve: (html: string) => void; reject: (error: Error) => void }
>();

// By source, for the page's life, so showing a card again or flipping it back doesn't convert it again
const converting = new Map<string, Promise<string>>();
const converted = new Map<string, string>();
let warned = false;

function stopWorker(): void {
  worker?.terminate();
  worker = null;
  config = null;
  for (const call of waiting.values())
    call.reject(new Error("pandoc was stopped"));
  waiting.clear();
}

/** The worker, started if it isn't, and kept for `IDLE_MS` more. */
function startWorker(): { worker: Worker; config: Promise<PandocConfig> } {
  if (worker === null) {
    worker = new Worker(new URL("./pandoc.worker.ts", import.meta.url), {
      type: "module",
    });
    worker.onmessage = ({ data }: MessageEvent<PandocResponse>) => {
      const call = waiting.get(data.id);
      waiting.delete(data.id);
      if ("html" in data) call?.resolve(data.html);
      else call?.reject(new Error(data.error));
    };
    worker.onerror = (event) => {
      event.preventDefault();
      stopWorker();
    };
  }
  config ??= call<PandocConfig>("pandoc_config").catch((e) => {
    config = null;
    throw e;
  });
  clearTimeout(stopTimer);
  stopTimer = setTimeout(stopWorker, IDLE_MS);
  return { worker, config };
}

async function convert(source: string): Promise<string> {
  const started = startWorker();
  const { version, web_defaults } = await started.config;
  const id = nextId++;
  const html = new Promise<string>((resolve, reject) =>
    waiting.set(id, { resolve, reject }),
  );
  started.worker.postMessage({
    id,
    source,
    wasmUrl: PANDOC_WASM_URL,
    cache: COPY_PANDOC_CACHE,
    serverVersion: version,
    webDefaults: web_defaults,
  } satisfies PandocRequest);
  return html;
}

/** A card's HTML as pandoc converts its Markdown `source`. */
export function pandocCardHtml(source: string): Promise<string> {
  let html = converting.get(source);
  if (html === undefined) {
    html = convert(source).then(
      (html) => {
        converted.set(source, html);
        return html;
      },
      (e) => {
        // Tried again the next time the card is shown, e.g. once pandoc.wasm is downloaded
        converting.delete(source);
        throw e;
      },
    );
    converting.set(source, html);
  }
  return html;
}

/** Converts a card that is likely to be shown next, e.g. a review card's back, so it shows at once. */
export function convertAhead(source: string): void {
  pandocCardHtml(source).catch(() => {
    // Shown with `marked` then
  });
}

/** `source`'s HTML from pandoc, or `null` until it comes, or if pandoc fails. */
export function usePandocHtml(source: string): string | null {
  const [result, setResult] = useState<{ source: string; html: string } | null>(
    null,
  );
  useEffect(() => {
    if (converted.has(source)) return;
    let cancelled = false;
    pandocCardHtml(source).then(
      (html) => {
        if (!cancelled) setResult({ source, html });
      },
      (e) => {
        if (warned) return;
        warned = true;
        console.warn(
          "Showing Markdown cards changed on this device without pandoc:",
          e,
        );
      },
    );
    return () => {
      cancelled = true;
    };
  }, [source]);
  return (
    converted.get(source) ?? (result?.source === source ? result.html : null)
  );
}
