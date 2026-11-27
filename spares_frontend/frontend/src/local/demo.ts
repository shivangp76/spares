// The demo's collection (see src/demo.ts): loaded into this browser's copy from the files
// demo/build.ts writes next to the app, where a device would download it from its host.
//
// - `collection.sqlite` goes in with spares_wasm's `load_snapshot`, as a full download would, with
//   the host's settings (`config.json`) and image occlusions.
// - The rendered files and render assets go in the copy's caches (`COPY_FILES_CACHE`,
//   `COPY_RENDER_ASSETS_CACHE`), under the URLs the API client looks them up by, as copy.ts
//   keeps what it downloads.
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

/** `manifest.json` (see demo/build.ts) */
interface Manifest {
  /** Unix time */
  built: number;
  replica: number;
  /** Rendered files, relative to the files directory */
  files: string[];
  assets: { path: string; hash: string }[];
  /** Image occlusion files, by name */
  imageOcclusions: string[];
}

const DEMO_DIR = `${BASE_URL}demo/`;
/** When the build of the demo whose collection this browser has was made, from its manifest */
const DEMO_BUILT_KEY = "spares_demo_built";
/** Files fetched at once */
const CONCURRENCY = 6;

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

/** `res` to keep in a cache, without the headers that describe how it was sent rather than its body */
async function keepable(res: Response): Promise<Response> {
  const headers = new Headers();
  const type = res.headers.get("Content-Type");
  if (type) headers.set("Content-Type", type);
  return new Response(await res.blob(), { headers });
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
  const tasks: (() => Promise<void>)[] = [
    ...manifest.files.map((path) => async () => {
      const res = await fetchOk(`files/${path.split("/").map(encodeURIComponent).join("/")}`);
      await filesCache.put(fileUrl(path), await keepable(res));
    }),
    ...manifest.assets.map(({ path, hash }) => async () => {
      const res = await fetchOk(`assets/${hash}`);
      await assetsCache.put(renderAssetUrl(path), await keepable(res));
    }),
  ];
  let next = 0;
  let done = 0;
  const step = "Loading cards";
  onProgress({ step, done, total: tasks.length });
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
    manifest.files.filter((path) => path.endsWith(".html")),
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
