// Converts the Markdown cards of notes changed on this device with pandoc (pandocWasm.ts), for
// pandoc.ts. A worker of its own: a card takes 20–150 ms, which would make the page jank, and the
// copy's worker answers the database calls a review waits on. pandoc starts once, at the first
// card, and answers every card after it.
import { parse } from 'yaml';
import { cardOptions, startPandoc, type Pandoc } from './pandocWasm';

export interface PandocRequest {
  id: number;
  source: string;
  /** pandoc.wasm, as the copy keeps it in `cache` */
  wasmUrl: string;
  cache: string;
  /** The server's pandoc version and web defaults file (`pandoc_config` in spares_wasm) */
  serverVersion: string | null;
  webDefaults: string | null;
}

export type PandocResponse = { id: number; html: string } | { id: number; error: string };

let pandoc: Promise<Pandoc> | null = null;
let options: { webDefaults: string | null; options: Record<string, unknown> } | null = null;

async function load({ wasmUrl, cache, serverVersion }: PandocRequest): Promise<Pandoc> {
  // Only the copy's own: it downloads it with its files, and a card shows as `marked` has it until then
  const response = await (await caches.open(cache)).match(wasmUrl);
  if (!response) throw new Error(`${wasmUrl} hasn't been downloaded yet`);
  const module = await WebAssembly.compileStreaming(response.clone())
    .catch(async () => WebAssembly.compile(await response.arrayBuffer()));
  const started = await startPandoc(module);
  // Still used: the server's render replaces its HTML after the next sync
  const version = started.version();
  if (serverVersion !== null && serverVersion !== version) {
    console.warn(`This device converts Markdown cards with pandoc ${version}, and the server renders them with ${serverVersion}.`);
  }
  return started;
}

function optionsFor(webDefaults: string | null): Record<string, unknown> {
  if (options?.webDefaults !== webDefaults) {
    const parsed = webDefaults === null ? null : parse(webDefaults) as unknown;
    if (parsed !== null && (typeof parsed !== 'object' || Array.isArray(parsed))) throw new Error('The web defaults file is not a YAML mapping');
    options = { webDefaults, options: cardOptions(parsed as Record<string, unknown> | null) };
  }
  return options.options;
}

self.onmessage = async ({ data }: MessageEvent<PandocRequest>) => {
  const { id } = data;
  try {
    // Tried again for the next card if it failed, e.g. before pandoc.wasm was downloaded
    pandoc ??= load(data).catch(e => { pandoc = null; throw e; });
    const converted = (await pandoc).convert(optionsFor(data.webDefaults), data.source);
    if (converted.stdout === '') throw new Error(converted.stderr.trim() || 'pandoc wrote nothing');
    self.postMessage({ id, html: converted.stdout } satisfies PandocResponse);
  } catch (e) {
    // pandoc may be left in a bad state by a trap, so the next card starts it again
    if (e instanceof WebAssembly.RuntimeError) pandoc = null;
    self.postMessage({ id, error: e instanceof Error ? e.message : String(e) } satisfies PandocResponse);
  }
};
