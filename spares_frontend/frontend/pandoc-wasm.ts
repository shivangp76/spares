// The pandoc.wasm a device that keeps a copy converts Markdown cards with (see src/local/pandoc.ts):
// pandoc's own WebAssembly build, of the version the server runs (`PANDOC_VERSION` in
// scripts/setup_vps.sh, which `spares deploy` installs). It is downloaded once from pandoc's
// releases into the cache, with its brotli and gzip copies, since compressing it takes a while.
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { brotliCompress, constants as zlibConstants, gzip } from "node:zlib";

const SETUP_VPS = fileURLToPath(
  new URL("../../scripts/setup_vps.sh", import.meta.url),
);

/** The server's pandoc version, e.g. `3.12.1`. */
export function pandocVersion(): string {
  const version = /^PANDOC_VERSION="([^"]+)"$/m.exec(
    readFileSync(SETUP_VPS, "utf8"),
  )?.[1];
  if (!version) throw new Error(`No PANDOC_VERSION in ${SETUP_VPS}`);
  return version;
}

/** Where pandoc.wasm of `version` is kept once downloaded, next to `spares deploy`'s pandoc package. */
export function cachedPandocWasm(version: string): string {
  const cache = process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache");
  return join(cache, "spares-deploy", `pandoc-${version}.wasm`);
}

/** pandoc.wasm of the server's version, with `.br` and `.gz` copies next to it, downloaded if it isn't yet. */
export async function pandocWasm(): Promise<{ version: string; path: string }> {
  const version = pandocVersion();
  const path = cachedPandocWasm(version);
  if (existsSync(path)) return { version, path };

  console.log(`Downloading pandoc ${version}'s WebAssembly build`);
  // Its name changes from release to release
  const release = await fetch(
    `https://api.github.com/repos/jgm/pandoc/releases/tags/${version}`,
  );
  if (!release.ok)
    throw new Error(
      `pandoc's release ${version}: ${release.status} ${await release.text()}`,
    );
  const assets = (
    (await release.json()) as {
      assets: { name: string; browser_download_url: string }[];
    }
  ).assets;
  const asset = assets.find(
    (asset) => asset.name.includes("wasm") && asset.name.endsWith(".zip"),
  );
  if (!asset)
    throw new Error(
      `pandoc ${version} has no WebAssembly build. Its releases have one from 3.9.`,
    );
  const zip = await fetch(asset.browser_download_url);
  if (!zip.ok) throw new Error(`${asset.browser_download_url}: ${zip.status}`);
  const dir = mkdtempSync(join(tmpdir(), "pandoc-wasm-"));
  let wasm: Buffer;
  try {
    writeFileSync(join(dir, asset.name), Buffer.from(await zip.arrayBuffer()));
    wasm = execFileSync(
      "unzip",
      ["-p", join(dir, asset.name), "*pandoc.wasm"],
      { maxBuffer: 512 * 1024 * 1024 },
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  if (wasm.length === 0) throw new Error(`${asset.name} has no pandoc.wasm`);

  console.log(`Compressing pandoc ${version}'s WebAssembly build`);
  const [br, gz] = await Promise.all([
    promisify(brotliCompress)(wasm, {
      params: {
        [zlibConstants.BROTLI_PARAM_QUALITY]: zlibConstants.BROTLI_MAX_QUALITY,
        [zlibConstants.BROTLI_PARAM_SIZE_HINT]: wasm.length,
      },
    }),
    promisify(gzip)(wasm, { level: 9 }),
  ]);
  mkdirSync(join(path, ".."), { recursive: true });
  // pandoc.wasm last, since it being there means the others are too
  for (const [file, data] of [
    [`${path}.br`, br],
    [`${path}.gz`, gz],
    [path, wasm],
  ] as const) {
    writeFileSync(`${file}.tmp`, data);
    renameSync(`${file}.tmp`, file);
  }
  return { version, path };
}
