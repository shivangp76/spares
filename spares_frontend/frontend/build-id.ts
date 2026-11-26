import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, resolve, sep } from "node:path";
import type { Plugin, ResolvedConfig } from "vite";

const BUILD_FILE = "build.json";

/** The id of a build, as the page and `build.json` have it (see `buildId`). */
export interface BuildInfo {
  build: string;
}

/** The directory a build writes to: `build.outDir` (`dist` unless `--outDir` says otherwise). */
export function outDirOf(config: ResolvedConfig): string {
  return resolve(config.root, config.build.outDir);
}

function listFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  });
}

/**
 * Names the build by a hash of its files, so building the same sources again gives the same id.
 * The page has it in `<meta name="spares-build">`, and `build.json` beside it, which the server
 * reads (see `spares_server::version`), so a page can tell it is older than the build the server
 * has (see src/version.ts). An installed web app, as on iOS, is often a page resumed from days ago
 * rather than loaded again.
 *
 * Runs before `serviceWorker`, whose own hash then covers the page with its id.
 */
export function buildId(): Plugin {
  let outDir: string;
  return {
    name: "build-id",
    apply: "build",
    configResolved(config) {
      outDir = outDirOf(config);
    },
    closeBundle: {
      order: "post",
      handler() {
        const files = listFiles(outDir)
          .map((path) => relative(outDir, path).split(sep).join("/"))
          .filter(
            (path) =>
              path !== "sw.js" &&
              path !== BUILD_FILE &&
              !/\.(br|gz)$/.test(path),
          )
          .sort();
        const hash = createHash("sha256");
        for (const path of files)
          hash.update(path).update(readFileSync(join(outDir, path)));
        const build = hash.digest("hex").slice(0, 12);
        const page = join(outDir, "index.html");
        const html = readFileSync(page, "utf8");
        writeFileSync(
          page,
          html.replace(
            "</head>",
            `  <meta name="spares-build" content="${build}" />\n  </head>`,
          ),
        );
        const info: BuildInfo = { build };
        writeFileSync(join(outDir, BUILD_FILE), JSON.stringify(info));
      },
    },
  };
}
