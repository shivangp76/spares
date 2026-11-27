// Builds the demo as far as it needs building, and serves it. Run it from anywhere in the
// repository with `cargo xtask demo`, or here with `bun demo/run.ts` (`bun run demo`):
//
//   bun demo/run.ts            # builds dist-demo/, serves it at http://127.0.0.1:8798/spares/
//   bun demo/run.ts --dev      # vite's dev server instead, which reloads as the app changes
//
// The collection (dist-demo/demo/, which build.ts makes in about two minutes) is made again only
// when there is none, when it is over a week old (its reviews are dated from when it was made, so
// cards stop being due), or when what it is made from changed since: notes.ts, build.ts, the
// images and audio clips the notes upload, or the database's migrations. Otherwise only the app
// is rebuilt, beside the collection it keeps. The new build replaces dist-demo/ only once it is complete.
//
// Options:
//   --fresh      make the collection again anyway
//   --app        keep the collection, however old
//   --dev        serve the app from vite's dev server, with the collection from dist-demo/demo/
//   --open       open the demo in a browser
//   --port <n>   the port to serve it on (8798)
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync } from "node:fs";
import { createServer } from "node:net";
import { join, relative, resolve } from "node:path";
import { parseArgs } from "node:util";
import { finishDemoBuild } from "./finish";
import { serveDemo } from "./serve";

const FRONTEND = resolve(import.meta.dir, "..");
const REPO = resolve(FRONTEND, "../..");
const DIST = join(FRONTEND, "dist-demo");
/** Where a build goes until it is complete */
const NEXT = join(FRONTEND, "dist-demo.next");
const COLLECTION = join(DIST, "demo");
const MANIFEST = join(COLLECTION, "manifest.json");
const BASE = "/spares/";
const MAX_AGE_DAYS = 7;
const DAY_MS = 24 * 60 * 60 * 1000;
/** The port of build.ts's host */
const BUILD_HOST_PORT = Number(process.env.SPARES_DEMO_PORT ?? 8797);
const BIN = join(FRONTEND, "node_modules/.bin");

function fail(message: string): never {
  console.error(`demo: ${message}`);
  process.exit(1);
}

let args: { fresh?: boolean; app?: boolean; dev?: boolean; open?: boolean; port?: string; help?: boolean };
try {
  ({ values: args } = parseArgs({
    options: {
      fresh: { type: "boolean" },
      app: { type: "boolean" },
      dev: { type: "boolean" },
      open: { type: "boolean" },
      port: { type: "string" },
      help: { type: "boolean", short: "h" },
    },
  }));
} catch (error) {
  fail(`${(error as Error).message}\nusage: bun demo/run.ts [--fresh | --app] [--dev] [--open] [--port <n>]`);
}
if (args.help) {
  console.log("usage: bun demo/run.ts [--fresh | --app] [--dev] [--open] [--port <n>]\nSee the top of demo/run.ts.");
  process.exit(0);
}
if (args.fresh && args.app) fail("--fresh makes the collection again, and --app keeps it: give one");
const port = Number(args.port ?? 8798);
if (!Number.isInteger(port) || port <= 0 || port > 65535) fail(`--port ${args.port} isn't a port`);

/** Whether nothing listens on `port` */
function portFree(port: number): Promise<boolean> {
  return new Promise((done) => {
    const server = createServer();
    server.once("error", () => done(false));
    server.listen(port, "127.0.0.1", () => server.close(() => done(true)));
  });
}

/** The files the collection is made from */
function collectionInputs(): string[] {
  const migrations = join(REPO, "spares_core/migrations");
  return [
    // With the images and audio clips the notes upload
    ...readdirSync(import.meta.dir)
      .filter((name) => ["notes.ts", "build.ts"].includes(name) || /\.(png|svg|mp3)$/.test(name))
      .map((name) => join(import.meta.dir, name)),
    ...readdirSync(migrations).map((name) => join(migrations, name)),
  ];
}

/** Why the collection needs making again, if it does */
function staleCollection(): string | null {
  if (!existsSync(MANIFEST)) return "there is none yet";
  const built = (JSON.parse(readFileSync(MANIFEST, "utf8")) as { built: number }).built * 1000;
  const days = (Date.now() - built) / DAY_MS;
  if (days > MAX_AGE_DAYS) return `it is ${Math.floor(days)} days old`;
  const changed = collectionInputs().filter((path) => statSync(path).mtimeMs > built);
  if (changed.length > 0) {
    const names = changed.slice(0, 2).map((path) => relative(REPO, path));
    if (changed.length > 2) names.push(`${changed.length - 2} more`);
    return `${names.join(", ")} changed since it was made`;
  }
  return null;
}

function step(what: string, command: string, argv: string[], options: { cwd?: string; env?: Record<string, string> } = {}): void {
  console.log(`\n» ${what}`);
  const result = spawnSync(command, argv, {
    cwd: options.cwd ?? FRONTEND,
    env: { ...process.env, ...options.env },
    stdio: "inherit",
  });
  if (result.error) fail(`${what} failed: ${result.error.message}`);
  if (result.status !== 0) fail(`${what} failed`);
}

if (!(await portFree(port)))
  fail(`port ${port} is in use, perhaps by another demo: stop it, or give another with --port`);
const stale = args.fresh ? "--fresh asks for it" : args.app ? null : staleCollection();
if (args.app && !existsSync(MANIFEST)) fail("there is no demo collection to keep: run it without --app");
if (stale && !(await portFree(BUILD_HOST_PORT)))
  fail(`port ${BUILD_HOST_PORT}, which the demo collection's build uses for its host, is in use`);

const bun = process.execPath;
step("Installing dependencies", bun, ["install", "--frozen-lockfile"]);
step("Building spares_wasm", bun, ["run", "build-wasm"]);
step("Preparing svgedit", bun, ["run", "prepare-svgedit"]);
if (stale) {
  console.log(`\nThe demo collection will be made again: ${stale}.`);
  step("Building spares and spares_server, which make it", "cargo", ["build", "-p", "spares_cli", "-p", "spares_server"], { cwd: REPO });
} else {
  console.log(`\nKeeping the demo collection in ${relative(FRONTEND, COLLECTION)} (--fresh makes it again).`);
}

rmSync(NEXT, { recursive: true, force: true });
if (!args.dev) {
  step("Type-checking", join(BIN, "tsc"), ["-b"]);
  step("Building the app", join(BIN, "vite"), ["build", "--base", BASE, "--outDir", NEXT, "--emptyOutDir"], {
    env: { VITE_SPARES_DEMO: "1" },
  });
}
if (stale) step("Making the demo collection", bun, ["demo/build.ts", "--out", NEXT]);

if (args.dev) {
  // Only the collection: the dev server serves the app
  if (stale) {
    mkdirSync(DIST, { recursive: true });
    rmSync(COLLECTION, { recursive: true, force: true });
    renameSync(join(NEXT, "demo"), COLLECTION);
    rmSync(NEXT, { recursive: true, force: true });
  }
} else {
  if (!stale) {
    finishDemoBuild(NEXT);
    renameSync(COLLECTION, join(NEXT, "demo"));
  }
  rmSync(DIST, { recursive: true, force: true });
  renameSync(NEXT, DIST);
}

const url = `http://127.0.0.1:${port}${BASE}`;
if (args.dev) {
  console.log(`\n» Serving the demo from vite's dev server at ${url}`);
  const vite = spawn(join(BIN, "vite"), ["--base", BASE, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: FRONTEND,
    env: { ...process.env, VITE_SPARES_DEMO: "1", ...(args.open ? { SPARES_OPEN: BASE } : {}) },
    stdio: "inherit",
  });
  vite.on("exit", (code) => process.exit(code ?? 0));
} else {
  serveDemo(DIST, port);
  console.log(`\nServing ${relative(FRONTEND, DIST)} at ${url} (Ctrl-C stops it)`);
  if (args.open) spawn(process.platform === "darwin" ? "open" : "xdg-open", [url], { stdio: "ignore" }).unref();
}
