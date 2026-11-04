// End-to-end test of a device's copy of the collection (src/local), in headless Chromium, WebKit
// and Firefox, against a server of its own on an empty collection:
//
// 1. Show a card's image occlusion without masks, from the server. Settings → Keep a Copy on This
//    Device.
// 2. Offline: review a card, and a card's image occlusion without masks, open `/`, see a card's
//    audio clip, add notes, one with a dropped image, edit a note the server also edits and one
//    with the server's image occlusion, search, open the statistics, undo. Show a Typst card with
//    the server's image occlusion, whose image the device makes.
// 3. Online again: sync, resolve the conflict on the Conflicts page, sync again. Add a note with a
//    new image occlusion, which the copy stores, and a Typst note with one, whose card shows before
//    a sync, and sync. Offline, a Markdown note with a cloze and an image occlusion, whose cards
//    show from their source, also after an edit, with `marked` since pandoc.wasm is deleted first.
//    Log out and back in without a reload: the copy syncs again, downloads pandoc.wasm again, and
//    shows the HTML the server rendered offline. A Markdown card made offline shows its math as
//    pandoc converts it.
// 4. The server has the device's notes, image and image occlusion, and the resolved text, and
//    rendered the device's Markdown note. Then Settings → Stop Keeping a Copy frees the device's
//    number.
// 5. In WebKit without a profile, as in a private window, Settings says it can't keep a copy.
//
// It needs a build (`bun run build`) and the browsers of playwright-core
// (`bunx playwright-core install chromium-headless-shell webkit firefox`). It builds spares_server,
// unless `SPARES_SERVER_BIN` names one.
//
//   bun e2e/local_copy.ts [chromium|webkit|firefox ...]
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateSync } from "node:zlib";
import {
  chromium,
  firefox,
  webkit,
  type BrowserContext,
  type Page,
} from "playwright-core";
import { formatNoteId } from "../src/utils";

const ROOT = join(import.meta.dir, "../../..");
const DIST = join(import.meta.dir, "../dist");
const PORT = Number(process.env.SPARES_E2E_PORT ?? 8796);
const ORIGIN = `http://127.0.0.1:${PORT}`;
const KEY = "e2e";
const ENGINES = { chromium, webkit, firefox };
const TIMEOUT = 120_000;

const engines = (
  process.argv.length > 2 ? process.argv.slice(2) : Object.keys(ENGINES)
) as (keyof typeof ENGINES)[];
for (const engine of engines)
  if (!(engine in ENGINES)) throw new Error(`Unknown browser ${engine}`);
if (!existsSync(join(DIST, "sw.js")))
  throw new Error(
    `Build the frontend first (bun run build): there is no ${DIST}`,
  );

// The server: built, and run on directories of its own, as the sync server (replica 1)

const SERVER_BIN =
  process.env.SPARES_SERVER_BIN ?? join(ROOT, "target/debug/spares_server");
if (!process.env.SPARES_SERVER_BIN) {
  const build = spawnSync("cargo", ["build", "-q", "-p", "spares_server"], {
    cwd: ROOT,
    stdio: "inherit",
  });
  if (build.status !== 0) throw new Error("Building spares_server failed");
}
const dir = mkdtempSync(join(tmpdir(), "spares-e2e-"));
for (const sub of ["config", "data", "files"]) mkdirSync(join(dir, sub));
// As on a VPS: Typst cards are compiled by browsers only
writeFileSync(
  join(dir, "config/config.toml"),
  "render_browser_parsers = false\n",
);
const env: Record<string, string | undefined> = { ...process.env };
// Settings of the user's own spares, such as where the laptop writes its files
for (const key of Object.keys(env))
  if (
    key.startsWith("SPARES_") ||
    key.endsWith("_OUT_DIR") ||
    key === "DATABASE_URL"
  )
    delete env[key];
Object.assign(env, {
  SPARES_CONFIG_DIR: join(dir, "config"),
  SPARES_DATA_DIR: join(dir, "data"),
  // One directory, as on a VPS, so the notes the server renders are the files it serves
  SPARES_CACHE_DIR: join(dir, "files"),
  SPARES_FILES_DIR: join(dir, "files"),
  DATABASE_URL: `sqlite://${join(dir, "data/spares.sqlite")}`,
  SPARES_SOCKET_ADDRESS: `127.0.0.1:${PORT}`,
  SPARES_SYNC_ROLE: "server",
  SPARES_REPLICA: "1",
  SPARES_API_KEY: KEY,
  SPARES_FRONTEND_DIR: DIST,
  RUST_LOG: process.env.RUST_LOG ?? "warn",
});
const server = spawn(SERVER_BIN, [], { env, stdio: "inherit" });

async function api<T>(
  method: string,
  path: string,
  body?: unknown,
): Promise<T> {
  const res = await fetch(`${ORIGIN}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${KEY}`,
      "Content-Type": "application/json",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok)
    throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
  const text = await res.text();
  return (text ? JSON.parse(text) : null) as T;
}

interface Note {
  id: number;
  data: string;
}

const IMAGE_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400"><rect width="400" height="400" fill="#F97316"/></svg>';
const CLOZES_SVG =
  '<svg width="400" height="400" xmlns="http://www.w3.org/2000/svg"><g class="layer" id="markup-group"><title>Markup</title></g><g class="layer" id="clozes-group"><title>Clozes</title><rect id="svg_1" x="10" y="10" width="50" height="50" fill="#FFEBA2"/></g></svg>';
/** With a shape on the Markup layer, which a card's back shows without masks too */
const MARKUP_CLOZES_SVG = CLOZES_SVG.replace(
  "<title>Markup</title>",
  '<title>Markup</title><circle id="svg_9" cx="200" cy="200" r="40" fill="#2563EB"/>',
);

/** A `width`×`height` PNG of one color. Image occlusions in cards need an image the `image` crate reads, which an SVG isn't. */
function png(
  width: number,
  height: number,
  [r, g, b]: [number, number, number],
): Buffer {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (data: Buffer) => {
    let c = 0xffffffff;
    for (const byte of data) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const typed = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc(typed));
    return Buffer.concat([length, typed, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.concat([
    Buffer.from([0]),
    Buffer.from(Array.from({ length: width }, () => [r, g, b]).flat()),
  ]);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk(
      "IDAT",
      deflateSync(Buffer.concat(Array.from({ length: height }, () => row))),
    ),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
const IMAGE_PNG = png(400, 400, [249, 115, 22]);

/** The names of the server's files of a kind (see `spares_core::sync::media`). */
async function serverMedia(
  kind: "image" | "image-occlusion",
): Promise<string[]> {
  return (await api<{ name: string }[]>("GET", `/api/sync/media/${kind}`)).map(
    (file) => file.name,
  );
}

async function serverNotes(query: string): Promise<Note[]> {
  const found = await api<{ Notes: [Note, string][] }>(
    "POST",
    "/api/notes/search",
    { query, output_type: "Notes" },
  );
  return found.Notes.map(([note]) => note);
}

let failed = 0;
function check(label: string, ok: boolean, detail = ""): void {
  if (!ok) failed += 1;
  console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail ? `: ${detail}` : ""}`);
}

async function seed(): Promise<void> {
  for (let i = 0; ; i++) {
    try {
      await api("GET", "/api/healthcheck");
      break;
    } catch (e) {
      if (i > 100) throw e;
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
  const typst = (
    await api<{ id: number }>("POST", "/api/parsers", { name: "typst" })
  ).id;
  const markdown = (
    await api<{ id: number }>("POST", "/api/parsers", { name: "markdown" })
  ).id;
  await api("POST", "/api/notes", {
    parser_id: typst,
    requests: [
      {
        data: "Seed: #cl[Paris] is the capital of #cl[France].",
        keywords: [],
        tags: [],
        is_suspended: false,
        custom_data: {},
      },
    ],
  });
  // A clip on the front, which a device's review plays too
  await api("POST", "/api/notes", {
    parser_id: markdown,
    requests: [
      {
        data: "Audio seed [listen](/clips/seed.mp3): {{Lyon}}",
        keywords: [],
        tags: [],
        is_suspended: false,
        custom_data: {},
      },
    ],
  });
  // An image occlusion made on the server, whose files the device downloads
  const form = new FormData();
  form.append("parser_id", String(markdown));
  form.append(
    "image",
    new File([IMAGE_PNG], "seed.png", { type: "image/png" }),
  );
  form.append("clozes", MARKUP_CLOZES_SVG);
  const res = await fetch(`${ORIGIN}/api/image-occlusions`, {
    method: "POST",
    headers: { Authorization: `Bearer ${KEY}` },
    body: form,
  });
  if (!res.ok)
    throw new Error(
      `POST /api/image-occlusions: ${res.status} ${await res.text()}`,
    );
  const { snippet } = (await res.json()) as { snippet: string };
  await api("POST", "/api/notes", {
    parser_id: markdown,
    requests: [
      {
        data: `Occlusion seed\n\n${snippet}`,
        keywords: [],
        tags: [],
        is_suspended: false,
        custom_data: {},
      },
    ],
  });
  // A Typst one, whose card's image this server never renders: the device makes it
  const typstForm = new FormData();
  typstForm.append("parser_id", String(typst));
  typstForm.append(
    "image",
    new File([IMAGE_PNG], "typst-seed.png", { type: "image/png" }),
  );
  typstForm.append("clozes", CLOZES_SVG);
  const typstRes = await fetch(`${ORIGIN}/api/image-occlusions`, {
    method: "POST",
    headers: { Authorization: `Bearer ${KEY}` },
    body: typstForm,
  });
  if (!typstRes.ok)
    throw new Error(
      `POST /api/image-occlusions: ${typstRes.status} ${await typstRes.text()}`,
    );
  const typstSnippet = ((await typstRes.json()) as { snippet: string }).snippet;
  await api("POST", "/api/notes", {
    parser_id: typst,
    requests: [
      {
        data: `Typst diagram seed\n\n${typstSnippet}`,
        keywords: [],
        tags: [],
        is_suspended: false,
        custom_data: {},
      },
    ],
  });
}

/** Opens the first card the Cards page finds for `query`, and waits for its front, a Typst card, to show an image, as the device makes it for an image occlusion. */
async function seeTypstCardImage(page: Page, query: string): Promise<boolean> {
  await page.goto(`${ORIGIN}/cards?q=${encodeURIComponent(query)}`);
  await page.locator("tbody tr").first().click();
  // A missing image fails the compile
  const shown = page
    .locator(".typst-viewer image")
    .or(page.getByText(/Error loading|failed|unavailable|doesn't have/i))
    .first();
  await shown.waitFor({ state: "attached", timeout: TIMEOUT });
  return (await page.locator(".typst-viewer image").count()) > 0;
}

type ShownCard = {
  marks: number;
  math: number;
  text: string;
  images: string[];
};

/** The front of each card the Cards page finds for `query`, a Markdown card the device shows from its source. Navigates within the app, since a reload offline is what other steps test. */
async function deviceMarkdownFronts(
  page: Page,
  query: string,
): Promise<ShownCard[]> {
  await page.getByRole("link", { name: "Cards", exact: true }).click();
  await page.getByPlaceholder(/^Search cards/).fill(query);
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await see(page, `results for "${query}"`);
  const rows = page.locator("tbody tr");
  await rows.first().waitFor();
  const count = await rows.count();
  const fronts: ShownCard[] = [];
  for (let i = 0; i < count; i++) {
    await rows.nth(i).click();
    await see(page, `Card ${i + 1} of ${count}`);
    await see(page, /shown from its text/);
    const frame = page.locator('iframe[title="Rendered card"]').first();
    // The previous card's front until this one's loads
    const shown = () =>
      frame.evaluate((iframe) => {
        const doc = (iframe as HTMLIFrameElement).contentDocument!;
        return {
          marks: doc.querySelectorAll("mark").length,
          math: doc.querySelectorAll("math").length,
          text: doc.body?.textContent ?? "",
          images: [...doc.images].map((image) => image.src),
        };
      });
    for (let tries = 0; ; tries++) {
      const front = await shown();
      if (
        (front.text.trim() !== "" &&
          (i === 0 || front.text !== fronts[i - 1].text)) ||
        tries === 50
      ) {
        fronts.push(front);
        break;
      }
      await page.waitForTimeout(200);
    }
  }
  return fronts;
}

/** The front of the first card the Cards page finds for `query`, once it shows math, as pandoc converts a Markdown card on the device, or after 30 s. */
async function deviceMathFront(page: Page, query: string): Promise<ShownCard> {
  await page.getByRole("link", { name: "Cards", exact: true }).click();
  await page.getByPlaceholder(/^Search cards/).fill(query);
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await see(page, `result for "${query}"`);
  await page.locator("tbody tr").first().click();
  await see(page, /shown from its text/);
  const frame = page.locator('iframe[title="Rendered card"]').first();
  for (let tries = 0; ; tries++) {
    const front = await frame.evaluate((iframe) => {
      const doc = (iframe as HTMLIFrameElement).contentDocument!;
      return {
        marks: doc.querySelectorAll("mark").length,
        math: doc.querySelectorAll("math").length,
        text: doc.body?.textContent ?? "",
        images: [],
      };
    });
    if (front.math > 0 || tries === 150) return front;
    await page.waitForTimeout(200);
  }
}

/** Text in the page, waited for. */
/**
 * Reviews the card `query` finds, and on its back opens its image occlusion without masks with
 * `m`: its original image with its markup over it, both loaded. Then `afterwards` closes it, or
 * rates the card. Whether they showed and the popup is gone.
 */
async function seeUnmasked(
  page: Page,
  query: string,
  afterwards: "Escape" | "1",
): Promise<boolean> {
  await page.goto(`${ORIGIN}/review?query=${encodeURIComponent(query)}`);
  await page.getByRole("button", { name: "Start Review" }).click();
  await page.getByRole("button", { name: /Show Answer/ }).click();
  await page.getByRole("button", { name: /Without Masks/ }).waitFor();
  await page.keyboard.press("m");
  const dialog = page.getByRole("dialog", { name: "Without masks" });
  await dialog.waitFor();
  const shown = await page
    .waitForFunction(
      () => {
        const images = [
          ...document.querySelectorAll<HTMLImageElement>(
            "dialog.unmasked-image-occlusions img",
          ),
        ];
        return (
          images.length === 2 &&
          images.every(
            (image) =>
              image.src.startsWith("blob:") &&
              image.complete &&
              image.naturalWidth > 0,
          )
        );
      },
      null,
      { timeout: TIMEOUT },
    )
    .then(
      () => true,
      () => false,
    );
  await page.keyboard.press(afterwards);
  await dialog.waitFor({ state: "detached" });
  // The next card, if any, starts with its masks
  if (afterwards === "1") await page.waitForTimeout(500);
  return shown && (await dialog.count()) === 0;
}

async function see(
  page: Page,
  text: string | RegExp,
  timeout = TIMEOUT,
): Promise<void> {
  await page.getByText(text).first().waitFor({ timeout });
}

/** Types `text` into the CodeMirror editor `editor`, replacing what it has. Again if keys were lost, as while the editor is set up for another parser. */
async function typeInto(
  page: Page,
  editor: ReturnType<Page["locator"]>,
  text: string,
): Promise<void> {
  for (let attempt = 1; ; attempt++) {
    await editor.click();
    await page.keyboard.press(
      process.platform === "darwin" ? "Meta+A" : "Control+A",
    );
    await page.keyboard.press("Backspace");
    // Typed as is: brackets the editor closes itself are typed over
    await page.keyboard.type(text);
    const typed = await editor.innerText();
    // An empty line reads as two line breaks
    const lines = (t: string) => t.trim().replace(/\n+/g, "\n");
    if (lines(typed) === lines(text)) return;
    if (attempt === 3)
      throw new Error(`Typing into the editor gave ${JSON.stringify(typed)}`);
    await page.waitForTimeout(500);
  }
}

/** Opens the New Note form with `parser` selected and `text` typed, and returns the form. */
async function startNote(
  page: Page,
  parser: string,
  text: string,
): Promise<ReturnType<Page["locator"]>> {
  await page.getByRole("button", { name: "New note" }).click();
  const form = page
    .locator("div")
    .filter({ has: page.getByRole("heading", { name: "New Note" }) })
    .last();
  const option = form
    .locator("select option")
    .filter({ hasText: new RegExp(`^${parser} `) })
    .first();
  await form
    .locator("select")
    .first()
    .selectOption((await option.getAttribute("value")) ?? "");
  await typeInto(page, form.locator(".cm-content").first(), text);
  return form;
}

async function createNote(
  page: Page,
  parser: string,
  text: string,
): Promise<void> {
  const form = await startNote(page, parser, text);
  await form.getByRole("button", { name: "Create" }).click();
  await see(page, /Note \w+ created\./);
}

/** Drops an image on the editor `editor`, and waits for the snippet that shows it. A drop, since
    Firefox leaves out the data of a paste event made by a script. */
async function dropImage(
  page: Page,
  editor: ReturnType<Page["locator"]>,
  name: string,
): Promise<void> {
  await editor.evaluate(
    (element, [svg, name]) => {
      const data = new DataTransfer();
      data.items.add(new File([svg], name, { type: "image/svg+xml" }));
      const { right, bottom } = element.getBoundingClientRect();
      element.dispatchEvent(
        new DragEvent("drop", {
          dataTransfer: data,
          clientX: right - 1,
          clientY: bottom - 1,
          bubbles: true,
          cancelable: true,
        }),
      );
    },
    [IMAGE_SVG, name],
  );
  await page.waitForFunction(
    (element) => /!\[\]\(\/[^)]*\/images\//.test(element!.textContent ?? ""),
    await editor.elementHandle(),
    { timeout: TIMEOUT },
  );
}

async function run(engine: keyof typeof ENGINES): Promise<void> {
  console.log(`\n${engine}`);
  const profile = mkdtempSync(join(dir, `${engine}-`));
  // A persistent context, since WebKit has no OPFS in an ephemeral one
  const context: BrowserContext = await ENGINES[engine].launchPersistentContext(
    profile,
    { headless: true },
  );
  context.setDefaultTimeout(TIMEOUT);
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log(`  [pageerror] ${e.message}`));
  if (process.env.SPARES_E2E_VERBOSE) {
    page.on("console", (message) =>
      console.log(`  [console.${message.type()}] ${message.text()}`),
    );
    page.on("requestfailed", (request) =>
      console.log(
        `  [requestfailed] ${request.url()}: ${request.failure()?.errorText}`,
      ),
    );
  }
  // Confirmations, e.g. of Undo last change and Stop Keeping a Copy
  page.on("dialog", (dialog) => dialog.accept());
  try {
    await page.goto(`${ORIGIN}/login`);
    await page.getByRole("heading", { name: "Spares" }).waitFor();
    const schedulers = await api<string[]>("GET", "/api/schedulers");
    await page.evaluate(
      (creds) => {
        localStorage.setItem("spares_credentials", JSON.stringify(creds));
        // Typed into the editor as into a plain one
        localStorage.setItem("spares_vim_keybindings", "false");
      },
      { serverUrl: ORIGIN, apiKey: KEY, schedulerName: schedulers[0] },
    );
    // Controlled by the service worker from here on, so the app opens offline
    await page.evaluate(() => navigator.serviceWorker.ready);
    // A card's image occlusion without masks, from the server
    check(
      "the back of a card shows its image occlusion without masks",
      await seeUnmasked(page, '"Occlusion seed"', "Escape"),
    );
    await page.goto(`${ORIGIN}/settings`);

    // 1. Set up the copy
    const started = performance.now();
    await page
      .getByRole("button", { name: "Keep a Copy on This Device" })
      .click();
    await see(page, /now keeps a copy of the collection/);
    check(
      "set up the copy",
      true,
      `${Math.round(performance.now() - started)} ms`,
    );
    const devices = await api<{ replica: number; name: string }[]>(
      "GET",
      "/api/sync/devices",
    );
    check(
      "the server has the device",
      devices.length === 1,
      JSON.stringify(devices),
    );
    await page.waitForFunction(
      () => document.body.innerText.match(/Downloading files/) === null,
      null,
      { timeout: TIMEOUT },
    );
    // Until the service worker has the modules the copy needs offline, and the image occlusion editor
    await page.reload();
    await page.waitForFunction(
      async () => {
        const keys = await (await caches.open("spares-assets")).keys();
        return (
          keys.some((request) => request.url.includes("spares_wasm_bg")) &&
          keys.some((request) =>
            request.url.includes("typst_ts_web_compiler"),
          ) &&
          (await caches.match("/svgedit/src/editor/index.html")) !== undefined
        );
      },
      null,
      { timeout: TIMEOUT, polling: 500 },
    );

    // A page put into the back-forward cache lets go of the copy, so the page after it can open
    // it, and opens it again when shown. The events are dispatched, since whether a page is cached
    // is the browser's choice.
    await page.evaluate(() =>
      window.dispatchEvent(
        new PageTransitionEvent("pagehide", { persisted: true }),
      ),
    );
    const next = await context.newPage();
    await next.goto(`${ORIGIN}/settings`);
    await next.getByRole("button", { name: "Stop Keeping a Copy" }).waitFor();
    const nextOpened = await next.evaluate(
      () =>
        !document.body.innerText.includes("Open in another tab") &&
        !document.querySelector('button[title*="Last sync failed"]'),
    );
    await next.close();
    await page.evaluate(() =>
      window.dispatchEvent(
        new PageTransitionEvent("pageshow", { persisted: true }),
      ),
    );
    const syncButton = page.getByRole("button", { name: /^Sync(?!ing)/ });
    await syncButton.click();
    await see(page, /Already in sync|Synced:/);
    const syncTitle = await syncButton.getAttribute("title");
    check(
      "the copy is let go of in the back-forward cache, and opened again",
      nextOpened && !syncTitle?.includes("failed"),
      syncTitle ?? "",
    );

    // 2. Offline
    await context.setOffline(true);
    check(
      "the back of a card shows its image occlusion without masks offline, and rating it closes them",
      await seeUnmasked(page, '"Occlusion seed"', "1"),
    );
    await page.goto(`${ORIGIN}/review`);
    await page.getByRole("button", { name: "Start Review" }).click();
    await page.getByRole("button", { name: /Show Answer/ }).click();
    await page.getByRole("button", { name: /\(1\)/ }).first().click();
    await see(page, /unsynced/);
    check("rated a card offline", true);

    // The address as typed, which other caches than the page's also have
    await page.goto(`${ORIGIN}/`);
    await see(page, /unsynced/);
    check("opened the server's address offline", true);

    await page.goto(
      `${ORIGIN}/review?query=${encodeURIComponent('"Audio seed"')}`,
    );
    await page.getByRole("button", { name: "Start Review" }).click();
    await page
      .locator('.audio-bar button[title="/clips/seed.mp3"]')
      .waitFor({ timeout: TIMEOUT });
    check("a card from the copy has its audio clip", true);

    await page.goto(`${ORIGIN}/notes`);
    const markdownText = `# Offline in ${engine}\n\nWritten on the device.\n\n<img src="x" onerror="window.parent.pwned = true">`;
    await createNote(page, "markdown", markdownText);
    await see(page, /Not rendered yet/);
    // In the sandbox rendered cards are shown in, since its raw HTML could otherwise run scripts
    const pendingHeading = page
      .frameLocator('iframe[title="Rendered card"]')
      .getByRole("heading", { name: `Offline in ${engine}` });
    await pendingHeading.waitFor({ timeout: TIMEOUT });
    check(
      "a Markdown note not rendered yet shows its text, sandboxed",
      !(await page.evaluate(() => "pwned" in window)),
    );
    // An image added offline, shown from the device until the server has it
    const imageForm = await startNote(page, "markdown", `# Image in ${engine}`);
    await dropImage(
      page,
      imageForm.locator(".cm-content").first(),
      `offline-${engine}.svg`,
    );
    await imageForm.getByRole("button", { name: "Create" }).click();
    await see(page, /Note \w+ created\./);
    await page
      .frameLocator('iframe[title="Rendered card"]')
      .locator('img[src^="blob:"]')
      .first()
      .waitFor({ timeout: TIMEOUT });
    check("added an image offline, shown from the device", true);

    await createNote(
      page,
      "typst",
      `Device note in ${engine}: #cl[Rome] is in #cl[Italy].`,
    );
    await createNote(
      page,
      "typst",
      `Undone in ${engine}: #cl[Bern] is in #cl[Switzerland].`,
    );
    await page.getByRole("button", { name: "Undo last change" }).click();
    await see(page, /Undone:/);
    check("undid a note offline", true);

    // The seed note, edited here and on the server
    const seedNote = (await serverNotes("Seed")).find((note) =>
      note.data.startsWith("Seed:"),
    )!;
    await api("PATCH", "/api/notes", {
      selector: { Ids: [seedNote.id] },
      data: `Seed: #cl[Paris] is the capital of #cl[France]. Server edit for ${engine}.`,
      tags: "None",
    });
    await page.getByPlaceholder("Search notes…").fill("Seed");
    await page.keyboard.press("Enter");
    await page
      .getByText(/^Seed:/)
      .first()
      .click();
    const editor = page.locator(".cm-content").first();
    await typeInto(
      page,
      editor,
      `Seed: #cl[Paris] is the capital of #cl[France]. Device edit for ${engine}.`,
    );
    // The note's, not the search's
    await page.locator("button.button-primary", { hasText: /^Save$/ }).click();
    await see(page, "Saved");

    // The server's image occlusion, parsed with the files the device downloaded
    await page.getByPlaceholder("Search notes…").fill("Occlusion seed");
    await page.keyboard.press("Enter");
    await page
      .getByRole("row")
      .filter({ hasText: "Occlusion seed" })
      .first()
      .click();
    await page
      .getByRole("button", { name: /Edit image occlusion 1/ })
      .waitFor();
    const occlusionEditor = page.locator(".cm-content").first();
    await occlusionEditor.click();
    await page.keyboard.press(
      process.platform === "darwin" ? "Meta+Home" : "Control+Home",
    );
    await page.keyboard.press("End");
    await page.keyboard.type(` (${engine})`);
    await page.locator("button.button-primary", { hasText: /^Save$/ }).click();
    await see(page, "Saved");
    check("edited a note with the server's image occlusion offline", true);

    await page.getByPlaceholder("Search notes…").fill(engine);
    await page.keyboard.press("Enter");
    await see(page, `Device note in ${engine}`);
    check(
      "searched offline",
      (await page.getByText(`Undone in ${engine}`).count()) === 0,
    );
    check(
      "a Typst card shows the server's image occlusion offline, made by the device",
      await seeTypstCardImage(page, '"Typst diagram seed"'),
    );

    await page.goto(`${ORIGIN}/statistics`);
    await page.waitForLoadState("networkidle");
    check(
      "opened the statistics offline",
      (await page.getByText(/failed/i).count()) === 0,
    );

    // 3. Online again
    // It syncs by itself once the connection is back, so the button may find nothing left to do
    await context.setOffline(false);
    await page.getByRole("button", { name: /^Sync/ }).click();
    await see(page, /Synced: received|Already in sync/);
    await page.getByRole("link", { name: "1", exact: true }).waitFor();
    check("synced, with a conflict", true);
    check(
      "online again",
      (await page.getByText("Offline", { exact: true }).count()) === 0,
    );
    await page.goto(`${ORIGIN}/conflicts`);
    await page.getByRole("button", { name: "Keep This Side" }).click();
    await see(page, /Resolved the conflict/);
    await page.getByRole("button", { name: /^Sync/ }).click();
    await see(page, /Synced: received \d+ changes, sent [1-9]/);

    // A new image occlusion, stored by the copy and uploaded with the note that names it
    const occlusionsBefore = await serverMedia("image-occlusion");
    await page.goto(`${ORIGIN}/notes`);
    const occlusionForm = await startNote(
      page,
      "markdown",
      `Occlusion from ${engine}`,
    );
    await occlusionForm
      .getByRole("button", { name: /Insert image occlusion/ })
      .click();
    const dialog = page.getByRole("dialog", { name: "Image occlusion editor" });
    // Once svgedit has loaded, which enables the file input
    await dialog.getByText(/Choose, paste or drop an image/).waitFor();
    await dialog.locator('input[type="file"]').setInputFiles({
      name: `made-${engine}.svg`,
      mimeType: "image/svg+xml",
      buffer: Buffer.from(IMAGE_SVG),
    });
    await dialog.getByText(/Shapes on the Clozes layer/).waitFor();
    await dialog.getByRole("button", { name: "Save and insert" }).click();
    await dialog.waitFor({ state: "detached" });
    check(
      "the server doesn't have the image occlusion before a note names it",
      (await serverMedia("image-occlusion")).length === occlusionsBefore.length,
    );
    await occlusionForm.getByRole("button", { name: "Create" }).click();
    await see(page, /Note \w+ created\./);
    await page.getByRole("button", { name: /^Sync/ }).click();
    await see(page, /Synced: received \d+ changes, sent [1-9]/);
    const madeOcclusion = (await serverMedia("image-occlusion")).filter(
      (name) => name.startsWith(`made_${engine}-`),
    );
    check(
      "uploaded the image occlusion with its note",
      madeOcclusion.length === 2,
      madeOcclusion.join(", "),
    );

    // A Typst card with an image occlusion made here, shown before the server has its note
    await page.goto(`${ORIGIN}/notes`);
    const typstOcclusionForm = await startNote(
      page,
      "typst",
      `Typst diagram from ${engine}`,
    );
    await typstOcclusionForm
      .getByRole("button", { name: /Insert image occlusion/ })
      .click();
    await dialog.getByText(/Choose, paste or drop an image/).waitFor();
    await dialog.locator('input[type="file"]').setInputFiles({
      name: `typst-${engine}.png`,
      mimeType: "image/png",
      buffer: IMAGE_PNG,
    });
    await dialog.getByText(/Shapes on the Clozes layer/).waitFor();
    // A cloze, as if drawn in svgedit, so the note has a card
    await page.locator('iframe[src*="/svgedit/"]').evaluate((frame, clozes) => {
      (
        frame as HTMLIFrameElement & {
          contentWindow: { sparesBridge: { getClozesSvg: () => string } };
        }
      ).contentWindow.sparesBridge.getClozesSvg = () => clozes;
    }, CLOZES_SVG);
    await dialog.getByRole("button", { name: "Save and insert" }).click();
    await dialog.waitFor({ state: "detached" });
    await typstOcclusionForm.getByRole("button", { name: "Create" }).click();
    await see(page, /Note \w+ created\./);
    check(
      "a Typst card shows an image occlusion made on the device before a sync",
      await seeTypstCardImage(page, `"Typst diagram from ${engine}"`),
    );

    // A Markdown note with a cloze and an image occlusion made here offline, whose cards show from their own source
    await context.setOffline(true);
    // As before the copy has downloaded it: cards show as `marked` has them, math as written
    await page.evaluate(() => caches.delete("spares-copy-pandoc"));
    await page.getByRole("link", { name: "Notes", exact: true }).click();
    const markdownOcclusionForm = await startNote(
      page,
      "markdown",
      `{{Paris}} is in France, said ${engine}. $x^2$\n\n`,
    );
    await markdownOcclusionForm
      .getByRole("button", { name: /Insert image occlusion/ })
      .click();
    await dialog.getByText(/Choose, paste or drop an image/).waitFor();
    await dialog.locator('input[type="file"]').setInputFiles({
      name: `markdown-${engine}.png`,
      mimeType: "image/png",
      buffer: IMAGE_PNG,
    });
    await dialog.getByText(/Shapes on the Clozes layer/).waitFor();
    await page.locator('iframe[src*="/svgedit/"]').evaluate((frame, clozes) => {
      (
        frame as HTMLIFrameElement & {
          contentWindow: { sparesBridge: { getClozesSvg: () => string } };
        }
      ).contentWindow.sparesBridge.getClozesSvg = () => clozes;
    }, CLOZES_SVG);
    await dialog.getByRole("button", { name: "Save and insert" }).click();
    await dialog.waitFor({ state: "detached" });
    await markdownOcclusionForm.getByRole("button", { name: "Create" }).click();
    await see(page, /Note \w+ created\./);
    const fronts = await deviceMarkdownFronts(page, `"said ${engine}"`);
    const clozeFront = fronts.find(
      (front) => front.marks > 0 && front.text.includes("is in France"),
    );
    check(
      "a Markdown card made offline hides its cloze, from its source",
      fronts.length === 2 &&
        clozeFront !== undefined &&
        !clozeFront.text.includes("Paris"),
      JSON.stringify(fronts),
    );
    check(
      "without pandoc.wasm, a Markdown card shows as `marked` has it",
      clozeFront !== undefined &&
        clozeFront.math === 0 &&
        clozeFront.text.includes("$x^2$"),
      JSON.stringify(clozeFront),
    );
    check(
      "a Markdown card made offline shows its image occlusion as the device makes it",
      fronts.some(
        (front) =>
          front.images.length > 0 &&
          front.images.every((src) => src.startsWith("blob:")),
      ),
      JSON.stringify(fronts),
    );
    await page.getByRole("link", { name: "Notes", exact: true }).click();
    await page.getByPlaceholder("Search notes…").fill(`"said ${engine}"`);
    await page.keyboard.press("Enter");
    await page
      .getByRole("row")
      .filter({ hasText: `said ${engine}` })
      .first()
      .click();
    await page
      .getByRole("button", { name: /Edit image occlusion 1/ })
      .waitFor();
    await page.locator(".cm-content").first().click();
    await page.keyboard.press(
      process.platform === "darwin" ? "Meta+Home" : "Control+Home",
    );
    await page.keyboard.press("End");
    await page.keyboard.type(" Edited offline.");
    await page.locator("button.button-primary", { hasText: /^Save$/ }).click();
    await see(page, "Saved");
    check(
      "a Markdown card shows its note's latest change made offline",
      (await deviceMarkdownFronts(page, `"said ${engine}"`)).every((front) =>
        front.text.includes("Edited offline."),
      ),
    );
    await context.setOffline(false);

    // Logging out keeps the copy and its files, and logging back in starts it again
    await page.getByRole("button", { name: "Logout" }).click();
    await page.getByLabel("Server URL").fill(ORIGIN);
    await page.getByLabel("API Key").fill(KEY);
    await page.getByRole("button", { name: "Login" }).click();
    await page.getByRole("button", { name: /^Sync/ }).click();
    await see(page, /Synced: received|Already in sync/);
    check("synced after logging out and back in", true);
    const rendered = (await serverNotes(engine)).find((note) =>
      note.data.startsWith("# Offline"),
    )!;
    await page.waitForFunction(
      async (letters) => {
        const keys = await (await caches.open("spares-copy-files")).keys();
        return keys.some(
          (request) =>
            request.url.includes(letters) && request.url.endsWith(".html"),
        );
      },
      formatNoteId(rendered.id),
      { timeout: TIMEOUT, polling: 500 },
    );
    // After the downloads, the web app tells the device which HTML it has
    await page
      .getByText(/Downloading files/)
      .waitFor({ state: "detached", timeout: TIMEOUT });
    await context.setOffline(true);
    await page.goto(`${ORIGIN}/notes`);
    await page.getByPlaceholder("Search notes…").fill(`Offline ${engine}`);
    await page.keyboard.press("Enter");
    await page
      .getByRole("row")
      .filter({ hasText: `# Offline in ${engine}` })
      .first()
      .click();
    // The HTML the server wrote next to the PDF
    await page
      .frameLocator("iframe")
      .getByRole("heading", { name: `Offline in ${engine}` })
      .waitFor({ timeout: TIMEOUT });
    check(
      "shows the rendered note offline, as HTML",
      (await page.getByText(/Not rendered yet|Error loading/i).count()) === 0,
    );
    // pandoc.wasm, downloaded again with the files
    await createNote(
      page,
      "markdown",
      `Math from ${engine}: $y_1$ and {{Paris}}\n\n$$\\int_0^1 x\\,dx$$\n`,
    );
    const mathFront = await deviceMathFront(page, `"Math from ${engine}"`);
    check(
      "a Markdown card made offline shows its math as MathML, with its cloze hidden",
      mathFront.math >= 2 &&
        mathFront.marks > 0 &&
        !mathFront.text.includes("Paris"),
      JSON.stringify(mathFront),
    );
    await context.setOffline(false);

    // 4. The server's collection
    const deviceNotes = await serverNotes(engine);
    const texts = deviceNotes.map((note) => note.data);
    check(
      "the server has the device notes",
      texts.some((text) => text.startsWith("Device note")) &&
        texts.some((text) => text.startsWith("# Offline")),
      JSON.stringify(texts),
    );
    check(
      "the undone note never reached the server",
      !texts.some((text) => text.startsWith("Undone")),
    );
    const seedAfter = (await serverNotes("Seed")).find((note) =>
      note.data.startsWith("Seed:"),
    )!;
    check(
      "the conflict resolution reached the server",
      seedAfter.data.includes(`Device edit for ${engine}`) &&
        !seedAfter.data.includes(`Server edit for ${engine}`),
      seedAfter.data,
    );
    const imageNote = deviceNotes.find((note) =>
      note.data.startsWith("# Image in"),
    );
    const images = await serverMedia("image");
    check(
      "the server has the image added offline",
      images.some(
        (name) =>
          name.startsWith(`offline_${engine}-`) &&
          imageNote?.data.includes(name),
      ),
      images.join(", "),
    );
    const occlusionNote = deviceNotes.find((note) =>
      note.data.startsWith("Occlusion from"),
    );
    const [occlusionSeed] = await serverNotes("Occlusion seed");
    check(
      "the server has the notes with image occlusions",
      occlusionNote !== undefined && occlusionSeed.data.includes(`(${engine})`),
      occlusionSeed.data,
    );
    const markdownNote = deviceNotes.find((note) =>
      note.data.startsWith("# Offline"),
    )!;
    const letters = formatNoteId(markdownNote.id);
    const files = (await api<{ path: string }[]>("GET", "/api/sync/files"))
      .map((file) => file.path)
      .filter((path) => path.includes(letters));
    check(
      "the server rendered the device's Markdown note",
      files.length > 0,
      `note ${letters}: ${files.join(", ")}`,
    );

    // A device that lost its copy without unenrolling, removed from this one's list
    await api("POST", "/api/sync/enroll", { name: "Lost phone" });
    await page.goto(`${ORIGIN}/settings`);
    await page.getByText("This device", { exact: true }).waitFor();
    await page
      .getByRole("listitem")
      .filter({ hasText: "Lost phone" })
      .getByRole("button", { name: "Remove" })
      .click();
    await page.getByText("Lost phone").waitFor({ state: "detached" });
    check(
      "removed a lost device",
      (await api<{ name: string }[]>("GET", "/api/sync/devices")).every(
        (device) => device.name !== "Lost phone",
      ),
    );

    await page.getByRole("button", { name: "Stop Keeping a Copy" }).click();
    await page
      .getByRole("button", { name: "Keep a Copy on This Device" })
      .waitFor();
    check(
      "stopped keeping a copy",
      (await api<unknown[]>("GET", "/api/sync/devices")).length === 0,
    );
    check(
      "stopping deleted pandoc.wasm",
      !(await page.evaluate(() => caches.has("spares-copy-pandoc"))),
    );
  } catch (e) {
    failed += 1;
    console.log(`FAIL ${engine}: ${e instanceof Error ? e.message : e}`);
    // Its title has the last sync's error
    const syncButton = await page
      .getByRole("button", { name: /^Sync/ })
      .getAttribute("title", { timeout: 1000 })
      .catch(() => null);
    if (syncButton) console.log(`  Sync button: ${syncButton}`);
    await page
      .screenshot({ path: join(dir, `${engine}-failure.png`) })
      .catch(() => {});
    console.log(`  screenshot: ${join(dir, `${engine}-failure.png`)}`);
  } finally {
    await context.close();
  }
}

/** WebKit without a profile, which has OPFS but refuses it, as a private window does: Settings says it can't keep a copy, rather than offering one. */
async function privateWindow(): Promise<void> {
  console.log("\nwebkit, without storage");
  const browser = await webkit.launch({ headless: true });
  try {
    const page = await browser.newPage();
    await page.goto(`${ORIGIN}/login`);
    await page.getByRole("heading", { name: "Spares" }).waitFor();
    const schedulers = await api<string[]>("GET", "/api/schedulers");
    await page.evaluate(
      (creds) =>
        localStorage.setItem("spares_credentials", JSON.stringify(creds)),
      { serverUrl: ORIGIN, apiKey: KEY, schedulerName: schedulers[0] },
    );
    await page.goto(`${ORIGIN}/settings`);
    await see(page, /can't keep a copy of the collection here/);
    check(
      "a window without storage says it can't keep a copy",
      (await page
        .getByRole("button", { name: "Keep a Copy on This Device" })
        .count()) === 0,
    );
  } catch (e) {
    failed += 1;
    console.log(
      `FAIL webkit without storage: ${e instanceof Error ? e.message : e}`,
    );
  } finally {
    await browser.close();
  }
}

try {
  await seed();
  for (const engine of engines) await run(engine);
  if (engines.includes("webkit")) await privateWindow();
} finally {
  server.kill();
}
if (failed === 0) rmSync(dir, { recursive: true, force: true });
else console.log(`\n${failed} failed. The server's directories: ${dir}`);
process.exit(failed === 0 ? 0 : 1);
