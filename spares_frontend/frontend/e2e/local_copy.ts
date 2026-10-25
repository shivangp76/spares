// End-to-end test of a device's copy of the collection (src/local), in headless Chromium, WebKit
// and Firefox, against a server of its own on an empty collection:
//
// 1. Settings → Keep a Copy on This Device.
// 2. Offline: review a card, add notes, edit a note the server also edits, search, open the
//    statistics, undo.
// 3. Online again: sync, resolve the conflict on the Conflicts page, sync again. Log out and back
//    in without a reload: the copy syncs again, and shows the HTML the server rendered offline.
// 4. The server has the device's notes and the resolved text, and rendered the device's Markdown
//    note. Then Settings → Stop Keeping a Copy frees the device's number.
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
  await api("POST", "/api/parsers", { name: "markdown" });
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
}

/** Text in the page, waited for. */
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

async function createNote(
  page: Page,
  parser: string,
  text: string,
): Promise<void> {
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
  await form.getByRole("button", { name: "Create" }).click();
  await see(page, /Note \w+ created\./);
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
    // Until the service worker has the modules the copy needs offline
    await page.reload();
    await page.waitForFunction(
      async () => {
        const keys = await (await caches.open("spares-assets")).keys();
        return (
          keys.some((request) => request.url.includes("spares_wasm_bg")) &&
          keys.some((request) => request.url.includes("typst_ts_web_compiler"))
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
    await page.goto(`${ORIGIN}/review`);
    await page.getByRole("button", { name: "Start Review" }).click();
    await page.getByRole("button", { name: /Show Answer/ }).click();
    await page.getByRole("button", { name: /\(1\)/ }).first().click();
    await see(page, /unsynced/);
    check("rated a card offline", true);

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
    const [seedNote] = await serverNotes("Seed");
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

    await page.getByPlaceholder("Search notes…").fill(engine);
    await page.keyboard.press("Enter");
    await see(page, `Device note in ${engine}`);
    check(
      "searched offline",
      (await page.getByText(`Undone in ${engine}`).count()) === 0,
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
    const [seedAfter] = await serverNotes("Seed");
    check(
      "the conflict resolution reached the server",
      seedAfter.data.includes(`Device edit for ${engine}`) &&
        !seedAfter.data.includes(`Server edit for ${engine}`),
      seedAfter.data,
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

try {
  await seed();
  for (const engine of engines) await run(engine);
} finally {
  server.kill();
}
if (failed === 0) rmSync(dir, { recursive: true, force: true });
else console.log(`\n${failed} failed. The server's directories: ${dir}`);
process.exit(failed === 0 ? 0 : 1);
