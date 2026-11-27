import { createHash } from "node:crypto";
import {
  copyFileSync,
  cpSync,
  createReadStream,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { writeFile } from "node:fs/promises";
import { basename, extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { brotliCompress, constants as zlibConstants, gzip } from "node:zlib";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { buildId, outDirOf } from "./build-id.ts";
import wasm from "vite-plugin-wasm";
import { pandocWasm } from "./pandoc-wasm.ts";

const SVGEDIT_EDITOR_DIR = fileURLToPath(
  new URL("./svgedit/src/editor", import.meta.url),
);

const IMAGE_OCCLUSION_TEMPLATE = fileURLToPath(
  new URL(
    "../../spares_core/src/parsers/image_occlusion/template.svg",
    import.meta.url,
  ),
);

const BACKGROUND_PATH = "/__spares/background";
const IMAGE_TYPES: Record<string, string> = {
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".bmp": "image/bmp",
};

/** Serves the standalone image occlusion editor its config (see `ext-spares/config.js`).
    `spares_frontend image-occlusion` sets it from the user's spares config, along with the image
    to open, if any. Otherwise, the editor gets the template and its own default settings. */
function imageOcclusionEditorConfig(): Plugin {
  return {
    name: "image-occlusion-editor-config",
    configureServer(server) {
      const background = process.env.SPARES_IMAGE_OCCLUSION_BACKGROUND;
      server.middlewares.use(
        "/__spares/image-occlusion-editor.json",
        (_req, res) => {
          const config = process.env.SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG
            ? JSON.parse(process.env.SPARES_IMAGE_OCCLUSION_EDITOR_CONFIG)
            : { template: readFileSync(IMAGE_OCCLUSION_TEMPLATE, "utf8") };
          if (background)
            config.background = {
              url: BACKGROUND_PATH,
              name: basename(background),
            };
          res.setHeader("Content-Type", "application/json");
          res.setHeader("Cache-Control", "no-store");
          res.end(JSON.stringify(config));
        },
      );
      // Only this one file is served, not the directory it is in
      server.middlewares.use(BACKGROUND_PATH, (_req, res) => {
        if (!background) {
          res.statusCode = 404;
          res.end();
          return;
        }
        res.setHeader(
          "Content-Type",
          IMAGE_TYPES[extname(background).toLowerCase()] ??
            "application/octet-stream",
        );
        res.setHeader("Cache-Control", "no-store");
        createReadStream(background)
          .on("error", () => {
            res.statusCode = 404;
            res.end();
          })
          .pipe(res);
      });
    },
  };
}

/** Whether this is the demo build or dev server (see src/demo.ts) */
const DEMO = process.env.VITE_SPARES_DEMO === "1";

const DEMO_TYPES: Record<string, string> = {
  ...IMAGE_TYPES,
  ".html": "text/html",
  ".json": "application/json",
  ".sqlite": "application/vnd.sqlite3",
  ".pdf": "application/pdf",
};

/** In the demo's dev server (`bun demo/run.ts --dev`), serves a collection that demo/build.ts
    made at `<base>demo/`, where src/local/demo.ts fetches it, as the demo build has it.
    `SPARES_DEMO_COLLECTION` is its directory, by default the one in dist-demo. */
function demoCollection(): Plugin {
  const dir = resolve(
    process.env.SPARES_DEMO_COLLECTION ??
      fileURLToPath(new URL("./dist-demo/demo", import.meta.url)),
  );
  return {
    name: "demo-collection",
    apply: (_config, { command }) => DEMO && command === "serve",
    configureServer(server) {
      if (!existsSync(join(dir, "manifest.json")))
        throw new Error(
          `${dir} has no demo collection: run \`bun demo/run.ts --dev\`, which builds one`,
        );
      const prefix = `${server.config.base}demo/`;
      server.middlewares.use((req, res, next) => {
        const pathname = (req.url ?? "").split("?")[0];
        if (!pathname.startsWith(prefix)) return next();
        const path = resolve(dir, decodeURIComponent(pathname.slice(prefix.length)));
        if (
          !path.startsWith(dir + sep) ||
          !existsSync(path) ||
          !statSync(path).isFile()
        ) {
          res.statusCode = 404;
          res.end();
          return;
        }
        res.setHeader(
          "Content-Type",
          DEMO_TYPES[extname(path).toLowerCase()] ?? "application/octet-stream",
        );
        res.setHeader("Cache-Control", "no-store");
        createReadStream(path).pipe(res);
      });
    },
  };
}

/** svgedit fetches its icons, shape libraries, etc. by URL at runtime, so they are copied next to
    its page rather than bundled. Its scripts are bundled from the entry. */
function copySvgeditAssets(): Plugin {
  let outDir: string;
  return {
    name: "copy-svgedit-assets",
    apply: "build",
    // Before the build starts: a build without svgedit fails, but the error it shows is from a
    // `closeBundle` hook that runs after the failure, such as the copy below
    config() {
      if (!existsSync(SVGEDIT_EDITOR_DIR))
        throw new Error(
          "svgedit is missing. Check out the submodule with `git submodule update --init`, or in " +
            "a jj workspace, where it is an empty directory, run " +
            "`scripts/setup_workspace.sh <main checkout>`.",
        );
    },
    configResolved(config) {
      outDir = outDirOf(config);
    },
    closeBundle() {
      for (const dir of ["images", "extensions"]) {
        cpSync(
          `${SVGEDIT_EDITOR_DIR}/${dir}`,
          `${outDir}/svgedit/src/editor/${dir}`,
          {
            recursive: true,
            filter: (source) => !source.endsWith(".js"),
          },
        );
      }
    },
  };
}

const SERVICE_WORKER_TEMPLATE = fileURLToPath(
  new URL("./service-worker.js", import.meta.url),
);

function listFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  });
}

/** Writes `sw.js` in the build from service-worker.js, with the files to cache for offline use. These are
    the page and everything it may load except the image occlusion editor and WebAssembly modules,
    which are large and only cached once used. The modules, having hashed names, are kept across
    builds that still have them. Devices that keep a copy of the collection cache the modules with
    the page, since they can't do without spares_wasm and the Typst compiler (see src/local), and
    the editor, so image occlusions can be inserted offline. */
function serviceWorker(): Plugin {
  let outDir: string;
  return {
    name: "service-worker",
    apply: "build",
    configResolved(config) {
      outDir = outDirOf(config);
    },
    // After `copySvgeditAssets`, so the build hash covers every file
    closeBundle: {
      order: "post",
      handler() {
        const files = listFiles(outDir)
          .map((path) => relative(outDir, path).split(sep).join("/"))
          .filter((path) => path !== "sw.js" && !COMPRESSED.test(path))
          .sort();
        const template = readFileSync(SERVICE_WORKER_TEMPLATE, "utf8");
        const hash = createHash("sha256").update(template);
        for (const path of files)
          hash.update(path).update(readFileSync(join(outDir, path)));
        const precache = files.filter(
          (path) => !path.startsWith("svgedit/") && !path.endsWith(".wasm"),
        );
        const keep = files.filter(
          (path) => path.startsWith("assets/") && path.endsWith(".wasm"),
        );
        const editor = files.filter((path) => path.startsWith("svgedit/"));
        const source = template
          .replace("__BUILD__", JSON.stringify(hash.digest("hex").slice(0, 16)))
          .replace("__PRECACHE__", JSON.stringify(precache, null, 2))
          .replace("__KEEP__", JSON.stringify(keep, null, 2))
          .replace("__EDITOR__", JSON.stringify(editor, null, 2));
        writeFileSync(join(outDir, "sw.js"), source);
      },
    },
  };
}

/** Puts pandoc.wasm of the server's version (see pandoc-wasm.ts) in the build, with its compressed
    copies, at a path naming the version, which the app gets as `__PANDOC_WASM__`. Only devices
    that keep a copy download it (see src/local/copy.ts), so it isn't precached, and the service
    worker leaves it to them. */
function pandocWasmFile(): Plugin {
  let wasm: { version: string; path: string };
  let url: string;
  let outDir: string;
  return {
    name: "pandoc-wasm",
    async config() {
      wasm = await pandocWasm();
      url = `/pandoc/pandoc-${wasm.version}.wasm`;
      return { define: { __PANDOC_WASM__: JSON.stringify(url) } };
    },
    configResolved(config) {
      outDir = outDirOf(config);
    },
    configureServer(server) {
      server.middlewares.use(url, (_req, res) => {
        res.setHeader("Content-Type", "application/wasm");
        createReadStream(wasm.path).pipe(res);
      });
    },
    // Before `serviceWorker`, so the build hash covers it, and `precompress`, which leaves it as it is
    closeBundle() {
      const out = join(outDir, url);
      mkdirSync(join(out, ".."), { recursive: true });
      for (const suffix of ["", ".br", ".gz"])
        copyFileSync(`${wasm.path}${suffix}`, `${out}${suffix}`);
    },
  };
}

// Compressed copies written by `precompress`
const COMPRESSED = /\.(br|gz)$/;
// Formats that are not compressed already, unlike images
const COMPRESSIBLE = /\.(html|js|css|svg|json|webmanifest|wasm|otf|ttf)$/;
// Brotli's best compression takes about a minute for the Typst compiler, for 10% less than this
const LARGE_FILE_BYTES = 1024 * 1024;
const LARGE_FILE_BROTLI_QUALITY = 9;

/** Writes brotli and gzip copies of the build's files (`foo.js.br`, `foo.js.gz`), which the server
    sends to browsers that accept them (see `create_router` in spares_server). */
function precompress(): Plugin {
  let outDir: string;
  const brotli = promisify(brotliCompress);
  const gzipAsync = promisify(gzip);
  return {
    name: "precompress",
    apply: "build",
    configResolved(config) {
      outDir = outDirOf(config);
    },
    // After `serviceWorker`, which lists the files to cache without these, so `sw.js` is compressed too
    closeBundle: {
      order: "post",
      async handler() {
        // Not files that come with their compressed copies, such as pandoc.wasm
        await Promise.all(
          listFiles(outDir)
            .filter(
              (path) => COMPRESSIBLE.test(path) && !existsSync(`${path}.br`),
            )
            .map(async (path) => {
              const data = readFileSync(path);
              if (data.length < 1024) return;
              const quality =
                data.length > LARGE_FILE_BYTES
                  ? LARGE_FILE_BROTLI_QUALITY
                  : zlibConstants.BROTLI_MAX_QUALITY;
              const [br, gz] = await Promise.all([
                brotli(data, {
                  params: {
                    [zlibConstants.BROTLI_PARAM_QUALITY]: quality,
                    [zlibConstants.BROTLI_PARAM_SIZE_HINT]: data.length,
                  },
                }),
                gzipAsync(data, { level: 9 }),
              ]);
              if (br.length < data.length) await writeFile(`${path}.br`, br);
              if (gz.length < data.length) await writeFile(`${path}.gz`, gz);
            }),
        );
      },
    },
  };
}

export default defineConfig({
  // The demo's dev server keeps its own prebundled dependencies, so that it doesn't rebuild those
  // of a dev server of the app running beside it
  cacheDir: DEMO ? "node_modules/.vite-demo" : undefined,
  optimizeDeps: {
    exclude: ["@myriaddreamin/typst.ts", "@myriaddreamin/typst-ts-renderer"],
  },
  assetsInclude: [
    "svgedit/src/editor/panels/*.html",
    "svgedit/src/editor/templates/*.html",
    "svgedit/src/editor/dialogs/*.html",
    "svgedit/src/editor/extensions/*/*.html",
  ],
  build: {
    rollupOptions: {
      // The image occlusion editor is its own page, embedded by the app in an iframe
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
        svgedit: fileURLToPath(
          new URL("./svgedit/src/editor/index.html", import.meta.url),
        ),
      },
    },
  },
  // This device's copy of the collection runs in a module worker (src/local/worker.ts)
  worker: {
    format: "es",
  },
  server: {
    // The demo's dev server opens a page only when asked (`bun demo/run.ts --dev --open`)
    open: process.env.SPARES_OPEN ?? (DEMO ? false : "/"),
    port: 5173,
    // The host's API and files, from this page's origin, so its session cookie works here too.
    // `SPARES_HOST` is the host to use, by default the one on this machine. The demo has no host.
    proxy: DEMO ? undefined : Object.fromEntries(
      ["/api", "/files"].map((path) => [
        path,
        {
          target: process.env.SPARES_HOST ?? "http://127.0.0.1:8080",
          changeOrigin: true,
        },
      ]),
    ),
  },
  plugins: [
    react(),
    wasm(),
    copySvgeditAssets(),
    pandocWasmFile(),
    buildId(),
    serviceWorker(),
    precompress(),
    imageOcclusionEditorConfig(),
    demoCollection(),
    {
      name: "html-import-transformer",
      transform(code, id) {
        // Only transform JS/TS files
        if (!id.match(/\.(js|ts|jsx|tsx)$/)) return;

        // Regex to match import statements with .html files
        // This handles both single and double quotes
        const htmlImportRegex =
          /(import\s+[^'"`]*?from\s+['"`].*?)\.html(['"`])/g;

        // Replace all matches by adding ?raw before the closing quote
        const transformedCode = code.replace(htmlImportRegex, "$1.html?raw$2");

        // Only return if we made changes
        if (transformedCode !== code) {
          return {
            code: transformedCode,
            map: null,
          };
        }
      },
    },
  ],
});
