// The demo's collection (see src/demo.ts): loaded into this browser's copy from the files
// demo/build.ts writes next to the app, where a device would download it from its host.
//
// - `collection.sqlite` goes in with spares_wasm's `load_snapshot`, as a full download would, with
//   the host's settings (`config.json`) and image occlusions.
// - The rendered files and render assets, which come in one file (`bundle.bin`), go in the copy's
//   caches (`COPY_FILES_CACHE`, `COPY_RENDER_ASSETS_CACHE`), under the URLs the API client looks
//   them up by, as copy.ts keeps what it downloads.
// - pandoc.wasm isn't kept: at 16 MB compressed, it would be most of what the demo downloads, so the
//   Markdown cards of notes changed here are shown from their source with `marked` (see pandoc.ts).
//
// The demo is rebuilt now and then, with its reviews dated from then. A browser that has an older
// build's collection replaces it with the new one (`DEMO_BUILT_KEY`), changes and all.
import { clearCaches, fileUrl, renderAssetUrl } from "../api/client";
import { BASE_URL, DEMO_ACCOUNT } from "../demo";
import { forgetCheckpoints } from "./copyCheckpoint";
import {
  call,
  canKeepCopy,
  COPY_FILES_CACHE,
  COPY_RENDER_ASSETS_CACHE,
  discardCopy,
  getLocalCopy,
  openCopy,
  setLocalCopy,
  stopCopy,
} from "./transport";

/** Where a file is in `bundle.bin`: bytes `start` to `end`, and its `Content-Type` */
interface BundleEntry {
  start: number;
  end: number;
  type: string;
}

/** `manifest.json` (see demo/build.ts) */
interface Manifest {
  /** Unix time */
  built: number;
  replica: number;
  /** Rendered files, by path relative to the files directory */
  files: ({ path: string } & BundleEntry)[];
  assets: ({ path: string; hash: string } & BundleEntry)[];
  /** Image occlusion files, by name */
  imageOcclusions: string[];
}

const DEMO_DIR = `${BASE_URL}demo/`;
/** When the build of the demo whose collection this browser has was made, from its manifest */
const DEMO_BUILT_KEY = "spares_demo_built";
/** Files put in the caches at once */
const CONCURRENCY = 16;

/** How far loading the demo has got, for the page shown meanwhile */
export interface DemoProgress {
  step: "Loading the collection" | "Loading cards";
  done: number;
  total: number;
}

async function fetchOk(path: string): Promise<Response> {
  const res = await fetch(DEMO_DIR + path);
  if (!res.ok) throw new Error(`Loading the demo's ${path} failed: ${res.status}`);
  return res;
}

/** The file `entry` describes in `bundle`, to keep in a cache as a host would have sent it */
function bundled(bundle: Blob, entry: BundleEntry): Response {
  return new Response(bundle.slice(entry.start, entry.end), {
    headers: { "Content-Type": entry.type },
  });
}

/**
 * Loads the demo's collection into this browser's copy, unless it has this build's already, and
 * returns when every page can use it. Throws if the browser can't keep a copy, as in a private
 * window.
 */
export async function loadDemo(
  onProgress: (progress: DemoProgress) => void,
): Promise<void> {
  let manifest: Manifest;
  try {
    manifest = (await (await fetchOk("manifest.json")).json()) as Manifest;
  } catch (e) {
    // Offline, the collection this browser has will do
    if (getLocalCopy()?.ready) return;
    throw e;
  }
  const built = String(manifest.built);
  if (getLocalCopy()?.ready) {
    if (localStorage.getItem(DEMO_BUILT_KEY) === built) return;
    await forgetDemo();
  }
  if (!(await canKeepCopy()))
    throw new Error(
      "This browser has no storage for the demo's collection, as in a private window. Open the demo in a normal window.",
    );
  onProgress({ step: "Loading the collection", done: 0, total: 1 });
  // The rendered files and assets, in one file (see demo/build.ts), fetched while the collection loads.
  // Named by the build, since a static host lets browsers keep a file for a while.
  const bundle = fetchOk(`bundle.bin?built=${built}`).then((res) => res.blob());
  bundle.catch(() => {});
  const state = (await openCopy()) as { replica: number | null };
  // Already loaded when an earlier attempt stopped after it
  if (state.replica === null) {
    const [snapshot, config, ...occlusions] = await Promise.all([
      fetchOk("collection.sqlite").then((res) => res.arrayBuffer()),
      fetchOk("config.json").then((res) => res.text()),
      ...manifest.imageOcclusions.map((name) =>
        fetchOk(`image-occlusions/${encodeURIComponent(name)}`).then((res) =>
          res.arrayBuffer(),
        ),
      ),
    ]);
    await call(
      "load_snapshot",
      new Uint8Array(snapshot),
      config,
      manifest.replica,
      manifest.imageOcclusions,
      occlusions.map((buffer) => new Uint8Array(buffer)),
    );
  }

  const [filesCache, assetsCache] = await Promise.all([
    caches.open(COPY_FILES_CACHE),
    caches.open(COPY_RENDER_ASSETS_CACHE),
  ]);
  const step = "Loading cards";
  const total = manifest.files.length + manifest.assets.length;
  onProgress({ step, done: 0, total });
  const files = await bundle;
  const tasks: (() => Promise<void>)[] = [
    ...manifest.files.map((entry) => () =>
      filesCache.put(fileUrl(entry.path), bundled(files, entry)),
    ),
    ...manifest.assets.map((entry) => () =>
      assetsCache.put(renderAssetUrl(entry.path), bundled(files, entry)),
    ),
  ];
  let next = 0;
  let done = 0;
  await Promise.all(
    Array.from({ length: Math.min(CONCURRENCY, tasks.length) }, async () => {
      while (next < tasks.length) {
        await tasks[next++]();
        done += 1;
        onProgress({ step, done, total: tasks.length });
      }
    }),
  );
  // Notes and cards are shown with their HTML only once the device says it has it
  await call(
    "web_renders",
    manifest.files.map(({ path }) => path).filter((path) => path.endsWith(".html")),
  );
  setLocalCopy({
    replica: manifest.replica,
    collectionId: DEMO_ACCOUNT,
    ready: true,
  });
  localStorage.setItem(DEMO_BUILT_KEY, built);
  clearCaches();
}

/** Deletes the demo's collection here, with every change made to it, and what the copy downloaded. */
async function forgetDemo(): Promise<void> {
  try {
    await call("forget");
  } catch {
    // A copy that can't be opened
    await discardCopy();
  }
  stopCopy();
  setLocalCopy(null);
  localStorage.removeItem(DEMO_BUILT_KEY);
  await forgetCheckpoints();
  for (const name of [COPY_FILES_CACHE, COPY_RENDER_ASSETS_CACHE])
    await caches.delete(name).catch(console.error);
  clearCaches();
}

/** Starts the demo over: deletes its collection here, with every change made to it, and reloads it. */
export async function resetDemo(): Promise<void> {
  await forgetDemo();
  window.location.reload();
}
