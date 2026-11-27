// The demo build: the web app with a collection of made-up notes and reviews in the browser, and
// no host behind it, so that it can be tried from a static site (GitHub Pages). Built with
// `bun run build-demo` (see demo/build.ts); src/local/demo.ts loads the collection.
//
// It runs like a device that keeps a copy of the collection (src/local/copy.ts), except that the
// copy comes from the demo's files rather than a host, never syncs, and is the only account.
// What only a host can do, such as rendering a note changed here or importing, says it isn't in
// the demo (`DEMO_NEEDS_SERVER`).

/** Whether this is the demo build (`VITE_SPARES_DEMO=1`) */
export const DEMO = import.meta.env.VITE_SPARES_DEMO === "1";

/** Where the app is served from, such as `/spares/` for the demo on GitHub Pages: `--base` of the build. */
export const BASE_URL = import.meta.env.BASE_URL;

/** `path`, a path of the app such as `/login`, under `BASE_URL`. */
export function appPath(path: string): string {
  return BASE_URL.replace(/\/$/, "") + path;
}

/** The message for something only a host does, in the demo. */
export function demoNeedsServer(what: string): string {
  return `${what} needs a spares host, so it isn't part of this demo. Install spares to try it.`;
}

/** The demo's one account, which its copy's storage is named after (see src/local/copyScope.ts) */
export const DEMO_ACCOUNT = "demo";

/** The demo's credentials, which src/local/copyScope.ts saves before anything reads them */
export const DEMO_CREDENTIALS = {
  // Nothing checks it: no request reaches a host
  csrfToken: "demo",
  user: "demo",
  accountId: DEMO_ACCOUNT,
  schedulerName: "fsrs",
};

/** Where the demo sends people who want spares itself */
export const REPOSITORY_URL = "https://github.com/shivangp76/spares";
