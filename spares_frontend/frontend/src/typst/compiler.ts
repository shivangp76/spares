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
import { fetchRenderAsset, listRenderPackages } from '../api/client';

const MAIN_FILE_PATH = '/main.typ';
// Each round can only discover the files read before the first missing one, e.g. a preamble and
// then the figures it includes.
const MAX_COMPILE_ROUNDS = 10;

/** Files the compiler asked for during the current compile that it did not have. */
const missingFiles = new Set<string>();
/** Files already requested from the server, whether or not it had them. */
const requestedFiles = new Set<string>();
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

let ready: Promise<void> | null = null;

function init(): Promise<void> {
  ready ??= (async () => {
    $typst.setCompilerInitOptions({ getModule: () => compilerWasmUrl });
    $typst.setRendererInitOptions({ getModule: () => rendererWasmUrl });
    const accessModel = new RecordingAccessModel();
    $typst.use(
      TypstSnippet.withAccessModel(accessModel),
      TypstSnippet.fetchPackageBy(accessModel, (_spec, url) =>
        packageTarballs.get(url) ?? fetchPackageSync(url)),
      // The browser has no system fonts, so fonts that sources set are bundled.
      TypstSnippet.preloadFonts([herosRegularUrl, herosBoldUrl, herosItalicUrl, herosBoldItalicUrl]),
      TypstSnippet.preloadFontAssets({ assets: ['text'] }),
    );
    const specs = await listRenderPackages();
    await Promise.allSettled(specs.map(prefetchPackage));
  })();
  // Allow retrying after e.g. a network error.
  ready.catch(() => { ready = null; });
  return ready;
}

async function compileNow(source: string): Promise<string> {
  await init();
  await $typst.addSource(MAIN_FILE_PATH, source);
  for (let round = 1; ; round++) {
    missingFiles.clear();
    try {
      return await $typst.svg({ mainFilePath: MAIN_FILE_PATH });
    } catch (e) {
      const newFiles = [...missingFiles].filter(path => !requestedFiles.has(path));
      if (newFiles.length === 0 || round >= MAX_COMPILE_ROUNDS) throw e;
      await Promise.all(newFiles.map(async path => {
        const data = await fetchRenderAsset(path);
        requestedFiles.add(path);
        if (data) await $typst.mapShadow(path, data);
      }));
    }
  }
}

// The compiler is shared and every compile writes the same main file, so compiles run one at a time.
let queue: Promise<unknown> = Promise.resolve();

/** Compiles `source` to an SVG string. Rejects with the compiler's diagnostics, a string. */
export function compileTypst(source: string): Promise<string> {
  const result = queue.then(() => compileNow(source));
  queue = result.catch(() => undefined);
  return result;
}
