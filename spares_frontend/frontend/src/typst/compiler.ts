// Compiles Typst sources in the browser so the server does not need a rendered file per card.
//
// Sources read files by absolute path (preambles, figures, image occlusions). The server stores
// copies of these, recorded with `typst compile --deps` when the cards were rendered locally. The
// files a source needs are only known by compiling it, so missing files are recorded by the access
// model, fetched, and the compile is retried.
//
// Packages cannot be discovered the same way since the compiler caches failed package lookups.
// The server lists every package rendered sources have imported, and those are prefetched. A
// package missing from that list is fetched synchronously as a fallback.
import { $typst, TypstSnippet } from '@myriaddreamin/typst.ts/contrib/snippet';
import { MemoryAccessModel } from '@myriaddreamin/typst.ts/fs/memory';
import compilerWasmUrl from '@myriaddreamin/typst-ts-web-compiler/wasm?url';
import rendererWasmUrl from '@myriaddreamin/typst-ts-renderer/wasm?url';
import herosRegularUrl from './fonts/texgyreheros-regular.otf?url';
import herosBoldUrl from './fonts/texgyreheros-bold.otf?url';
import herosItalicUrl from './fonts/texgyreheros-italic.otf?url';
import herosBoldItalicUrl from './fonts/texgyreheros-bolditalic.otf?url';
import { fetchRenderAsset, listRenderPackages, listRenderSources } from '../api/client';
import { errorMessage } from '../utils';

// Typst's own fonts, which the service worker keeps (see service-worker.js): New Computer Modern,
// for text and math, and DejaVu Sans Mono, for code. Not its default text font, Libertinus Serif,
// since templates set their own and it is 1.4 MB of the 8.7 MB they would otherwise add up to.
const TYPST_FONTS_URL = 'https://cdn.jsdelivr.net/gh/typst/typst-assets@v0.13.1/files/fonts/';
const TYPST_FONT_URLS = [
  'NewCM10-Regular.otf',
  'NewCM10-Bold.otf',
  'NewCM10-Italic.otf',
  'NewCM10-BoldItalic.otf',
  'NewCMMath-Regular.otf',
  'NewCMMath-Book.otf',
  'NewCMMath-Bold.otf',
  'DejaVuSansMono.ttf',
  'DejaVuSansMono-Bold.ttf',
  'DejaVuSansMono-Oblique.ttf',
  'DejaVuSansMono-BoldOblique.ttf',
].map(file => TYPST_FONTS_URL + file);

const MAIN_FILE_PATH = '/main.typ';
// Each round can only discover the files read before the first missing one, e.g. a preamble and
// then the figures it includes.
const MAX_COMPILE_ROUNDS = 10;

/** Files the compiler asked for during the current compile that it did not have. */
const missingFiles = new Set<string>();
/** Files already requested from the server, whether or not it had them. */
const requestedFiles = new Set<string>();
/** Requested files the server did not have. */
const unavailableFiles = new Set<string>();
const packageTarballs = new Map<string, Uint8Array>();

// Package files are extracted under `/@memory/`. Every other path is a local file.
class RecordingAccessModel extends MemoryAccessModel {
  getMTime(path: string): Date | undefined {
    const mtime = super.getMTime(path);
    if (mtime === undefined && !path.startsWith('/@memory/')) missingFiles.add(path);
    return mtime;
  }

  readAll(path: string): Uint8Array | undefined {
    const data = super.readAll(path);
    if (data === undefined && !path.startsWith('/@memory/')) missingFiles.add(path);
    return data;
  }
}

function packageUrl(spec: string): string {
  const [namespace, name, version] = spec.split('/');
  return `https://packages.typst.org/${namespace}/${name}-${version}.tar.gz`;
}

async function prefetchPackage(spec: string): Promise<void> {
  const url = packageUrl(spec);
  const res = await fetch(url);
  if (res.ok) packageTarballs.set(url, new Uint8Array(await res.arrayBuffer()));
}

function fetchPackageSync(url: string): Uint8Array | undefined {
  const request = new XMLHttpRequest();
  request.overrideMimeType('text/plain; charset=x-user-defined');
  request.open('GET', url, false);
  request.send(null);
  if (request.status !== 200 || typeof request.response !== 'string') return undefined;
  const data = Uint8Array.from(request.response, (c: string) => c.charCodeAt(0));
  packageTarballs.set(url, data);
  return data;
}

let configured = false;

/** Sets up `$typst`. Only once: setting it up again registers a second access model, which fails every compile. */
function configure(): void {
  if (configured) return;
  configured = true;
  $typst.setCompilerInitOptions({ getModule: () => compilerWasmUrl });
  $typst.setRendererInitOptions({ getModule: () => rendererWasmUrl });
  const accessModel = new RecordingAccessModel();
  $typst.use(
    TypstSnippet.withAccessModel(accessModel),
    TypstSnippet.fetchPackageBy(accessModel, (_spec, url) =>
      packageTarballs.get(url) ?? fetchPackageSync(url)),
    // The browser has no system fonts, so fonts that sources set are bundled.
    TypstSnippet.preloadFonts([herosRegularUrl, herosBoldUrl, herosItalicUrl, herosBoldItalicUrl, ...TYPST_FONT_URLS]),
    // Otherwise the compiler adds all of Typst's text fonts
    TypstSnippet.disableDefaultFontAssets(),
  );
}

let packagesPrefetched: Promise<void> | null = null;

function prefetchPackages(): Promise<void> {
  packagesPrefetched ??= (async () => {
    const specs = await listRenderPackages();
    await Promise.allSettled(specs.map(prefetchPackage));
  })();
  // Allow retrying after e.g. a network error.
  packagesPrefetched.catch(() => { packagesPrefetched = null; });
  return packagesPrefetched;
}

/** Gives the compiler `path`, as fetched from the server, or records that the server does not have it. */
async function addFile(path: string, data: Uint8Array | null): Promise<void> {
  requestedFiles.add(path);
  if (data) await $typst.mapShadow(path, data);
  else unavailableFiles.add(path);
}

let sourcesLoaded: Promise<void> | null = null;

/**
 * Gives the compiler the Typst files most sources read, e.g. preambles, so that compiles do not
 * each have to fail first to discover them.
 */
function loadSources(): Promise<void> {
  sourcesLoaded ??= (async () => {
    const paths = (await listRenderSources()).filter(path => !requestedFiles.has(path));
    const files = await Promise.all(paths.map(async path => [path, await fetchRenderAsset(path)] as const));
    // One at a time, since the first creates the compiler
    for (const [path, data] of files) await addFile(path, data);
  })();
  // Allow retrying after e.g. a network error.
  sourcesLoaded.catch(() => { sourcesLoaded = null; });
  return sourcesLoaded;
}

async function init(): Promise<void> {
  configure();
  const [packages, sources] = await Promise.allSettled([prefetchPackages(), loadSources()]);
  // Sources that import no packages still compile, and a package missing here is fetched when
  // the compiler asks for it. Likewise a file missing here. Both are retried on the next compile.
  if (packages.status === 'rejected') console.error('Prefetching Typst packages', packages.reason);
  if (sources.status === 'rejected') console.error('Loading Typst sources', sources.reason);
}

async function compileNow(source: string, inputs: Record<string, string>): Promise<string> {
  await init();
  await $typst.addSource(MAIN_FILE_PATH, source);
  const missing = new Set<string>();
  for (let round = 1; ; round++) {
    missingFiles.clear();
    try {
      return await $typst.svg({ mainFilePath: MAIN_FILE_PATH, inputs });
    } catch (e) {
      missingFiles.forEach(path => missing.add(path));
      const newFiles = [...missingFiles].filter(path => !requestedFiles.has(path));
      if (newFiles.length === 0 || round >= MAX_COMPILE_ROUNDS) throw withUnavailableFiles(e, source, missing);
      await Promise.all(newFiles.map(async path => addFile(path, await fetchRenderAsset(path))));
    }
  }
}

/**
 * `error`, the diagnostics of compiling `source`, naming the files it read that the server does not
 * have. The compiler reports a file it cannot read as "access denied", which does not say which.
 * It only asks for such a file once, so later compiles find it in `source` instead of `missing`.
 */
function withUnavailableFiles(error: unknown, source: string, missing: Set<string>): unknown {
  const unavailable = [...unavailableFiles].filter(path => missing.has(path) || source.includes(`"${path}"`));
  if (unavailable.length === 0) return error;
  const files = unavailable.map(path => `  ${path}`).join('\n');
  return `The server has no copy of these files the source reads:\n${files}\n\n${errorMessage(error)}`;
}

// The compiler is shared and every compile writes the same main file, so compiles run one at a time.
// Those in the background, e.g. of cards downloaded for offline review, wait for the others.
const tasks: (() => Promise<void>)[] = [];
const backgroundTasks: (() => Promise<void>)[] = [];
let running = false;

function enqueue<T>(task: () => Promise<T>, background = false): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    (background ? backgroundTasks : tasks).push(() => Promise.resolve().then(task).then(resolve, reject));
    void runTasks();
  });
}

async function runTasks(): Promise<void> {
  if (running) return;
  running = true;
  let task;
  while ((task = tasks.shift() ?? backgroundTasks.shift())) await task();
  running = false;
}

/**
 * Loads the compiler with its fonts and the renderer, and prefetches packages, ahead of the first
 * compile, e.g. before a review starts. Otherwise the first compile loads them.
 */
export function warmUpTypst(): Promise<void> {
  // Queued, since `$typst` creates a second compiler if one is asked for while the first loads
  return enqueue(async () => {
    await init();
    await Promise.all([$typst.getCompiler(), $typst.getRenderer()]);
  });
}

/** Results compiled ahead of being shown, by source and inputs. Only the latest few are kept. */
const precompiled = new Map<string, Promise<string>>();
const MAX_PRECOMPILED = 4;

const resultKey = (source: string, inputs: Record<string, string>) => JSON.stringify([source, inputs]);

/**
 * Compiles `source` ahead of it being shown, e.g. the next card while the review's filter is
 * picked, so that `compileTypst` returns the result at once.
 */
export function precompileTypst(source: string, inputs: Record<string, string> = {}): Promise<string> {
  const key = resultKey(source, inputs);
  const existing = precompiled.get(key);
  if (existing) return existing;
  const result = enqueue(() => compileNow(source, inputs));
  precompiled.set(key, result);
  // A failure, e.g. without a connection, is not kept, so showing the source compiles it again
  result.catch(() => { if (precompiled.get(key) === result) precompiled.delete(key); });
  for (const old of precompiled.keys()) {
    if (precompiled.size <= MAX_PRECOMPILED) break;
    precompiled.delete(old);
  }
  return result;
}

/** Compiles `source` after the compiles of sources being shown, e.g. to fetch the files it reads. */
export function compileTypstInBackground(source: string): Promise<string> {
  return enqueue(() => compileNow(source, {}), true);
}

/**
 * Compiles `source` to an SVG string. Rejects with the compiler's diagnostics, a string.
 *
 * `inputs` are visible to the source through `sys.inputs`. They are always passed since the
 * compiler otherwise reuses the inputs of the previous compile.
 */
export function compileTypst(source: string, inputs: Record<string, string> = {}): Promise<string> {
  return precompiled.get(resultKey(source, inputs)) ?? enqueue(() => compileNow(source, inputs));
}
