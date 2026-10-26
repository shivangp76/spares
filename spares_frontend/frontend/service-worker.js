// Lets the app open without a network connection. Built into `dist/sw.js` by the `service-worker`
// plugin in vite.config.ts, which fills in the build hash and the files to precache and keep.
//
// Only the app itself is cached here. Data from the server (`/api`, `/files`) is cached by the app
// (see src/offline), which knows what to keep for offline review and when it is stale.

const BUILD = __BUILD__;
// The page and the scripts, styles and fonts it loads, relative to this file
const PRECACHE = __PRECACHE__;
// Files of this build cached when first loaded, e.g. the Typst compiler, which are kept across
// builds that have them since their names are hashed
const KEEP = __KEEP__;
// The image occlusion editor's page and the files it loads by URL, whose names aren't hashed
const EDITOR = __EDITOR__;
// A device that keeps a copy of the collection needs those too, such as spares_wasm, to work
// offline after an update, so it caches them with the page, and the editor with this build's page
// so image occlusions can be inserted offline. The app marks such a device with this cache (see
// src/local/copy.ts).
const LOCAL_COPY_MARKER = "spares-local-copy";

// Build files have hashed names, so they never change and are kept across builds until the build
// that drops them takes over. The page itself is replaced on every build.
const ASSETS_CACHE = "spares-assets";
const PAGE_CACHE = `spares-page-${BUILD}`;
// Typst packages are versioned, so they never change either
const PACKAGES_CACHE = "spares-typst-packages";
const PACKAGES_ORIGIN = "https://packages.typst.org";
// Likewise Typst's fonts, which the compiler loads from a versioned URL (see src/typst/compiler.ts)
const FONTS_CACHE = "spares-typst-fonts";
const FONTS_URL = "https://cdn.jsdelivr.net/gh/typst/typst-assets@";

const PAGE_URL = new URL("index.html", self.registration.scope).href;
const precacheUrls = PRECACHE.map(
  (path) => new URL(path, self.registration.scope).href,
);
const keepUrls = KEEP.map(
  (path) => new URL(path, self.registration.scope).href,
);
const editorUrls = EDITOR.map(
  (path) => new URL(path, self.registration.scope).href,
);

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const page = await caches.open(PAGE_CACHE);
      await page.add(new Request(PAGE_URL, { cache: "reload" }));
      const localCopy = await caches.has(LOCAL_COPY_MARKER);
      if (localCopy) await cacheEditor(page);
      const assets = await caches.open(ASSETS_CACHE);
      const cached = new Set(
        (await assets.keys()).map((request) => request.url),
      );
      const urls = localCopy ? [...precacheUrls, ...keepUrls] : precacheUrls;
      await assets.addAll(
        urls.filter((url) => url !== PAGE_URL && !cached.has(url)),
      );
    })(),
  );
});

// A new build takes over once no page uses the old one, so the old page can still load its
// lazily loaded chunks until then. That is also when the files only the old build used are dropped.
self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      for (const name of await caches.keys()) {
        if (name.startsWith("spares-page-") && name !== PAGE_CACHE)
          await caches.delete(name);
      }
      const assets = await caches.open(ASSETS_CACHE);
      const current = new Set([...precacheUrls, ...keepUrls]);
      for (const request of await assets.keys()) {
        // Other files cached when first loaded (e.g. files without a hash in their name) are dropped
        // too, and fetched again the next time they are needed.
        if (!current.has(request.url)) await assets.delete(request);
      }
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("message", (event) => {
  if (event.data === "skipWaiting") self.skipWaiting();
  // From a page that set up a copy of the collection
  if (event.data === "cacheLocalCopy") {
    event.waitUntil(
      (async () => {
        const assets = await caches.open(ASSETS_CACHE);
        const cached = new Set(
          (await assets.keys()).map((request) => request.url),
        );
        await assets.addAll(keepUrls.filter((url) => !cached.has(url)));
        await cacheEditor(await caches.open(PAGE_CACHE));
      })(),
    );
  }
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);

  if (url.origin === PACKAGES_ORIGIN) {
    event.respondWith(cacheFirst(PACKAGES_CACHE, request));
    return;
  }
  if (request.url.startsWith(FONTS_URL)) {
    event.respondWith(cacheFirst(FONTS_CACHE, request));
    return;
  }
  if (url.origin !== self.location.origin) return;
  // The server's data, which the app caches itself
  if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/files/"))
    return;

  if (request.mode === "navigate") {
    event.respondWith(navigate(request));
    return;
  }
  event.respondWith(cacheFirst(ASSETS_CACHE, request));
});

/**
 * The page from the server when online, so a new build is picked up, and this build's page
 * otherwise. Every app route is served the same page, which routes on the client.
 */
async function navigate(request) {
  try {
    return await fetch(request);
  } catch (error) {
    // The image occlusion editor is its own page, whose settings are in its query, and is available
    // offline only on a device that keeps a copy
    const editor = new URL(request.url).pathname.startsWith("/svgedit/");
    const cached = await caches.match(request, { ignoreSearch: editor });
    if (cached) return cached;
    if (!editor) {
      const page = await caches.match(PAGE_URL);
      if (page) return page;
    }
    throw error;
  }
}

/** The editor's files, which change with the build since their names aren't hashed. */
async function cacheEditor(page) {
  const cached = new Set((await page.keys()).map((request) => request.url));
  await page.addAll(
    editorUrls
      .filter((url) => !cached.has(url))
      .map((url) => new Request(url, { cache: "no-cache" })),
  );
}

async function cacheFirst(cacheName, request) {
  const cache = await caches.open(cacheName);
  // Or this build's page cache, which has the editor's files on a device that keeps a copy
  const cached =
    (await cache.match(request)) ??
    (await caches.match(request, { cacheName: PAGE_CACHE }));
  if (cached) return cached;
  const response = await fetch(request);
  if (response.ok) await cache.put(request, response.clone());
  return response;
}
