// Dictionary lookup on the cards of the `markdown-language-tools` parser, on a disposable host,
// in Chromium, WebKit and Firefox: `bun e2e/language_tools.ts [chromium|webkit|firefox]`, after
// building the frontend and server. Not in `bun run e2e`; CI runs it weekly. With
// SPARES_E2E_SHOTS=<dir>, it saves screenshots of each step there.
//
// The cards' HTML is written as pandoc writes it (the Rust tests render it for real), and a small
// Yomitan dictionary is imported through the API.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, firefox, webkit, type Page } from "playwright-core";
import { formatNoteId } from "../src/utils";

const root = mkdtempSync(join(tmpdir(), "spares-language-tools-"));
const bin =
  process.env.SPARES_SERVER_BIN ??
  join(import.meta.dir, "../../../target/debug/spares_server");
const dist =
  process.env.SPARES_FRONTEND_DIST ?? join(import.meta.dir, "../dist");
const port = Number(process.env.SPARES_E2E_PORT ?? 18798);
const origin = `http://127.0.0.1:${port}`;
const shots = process.env.SPARES_E2E_SHOTS;
const env = { ...process.env, SPARES_ROOT: root };
const user = "lang";
const password = "language-password";

/** A zip of `files`, stored without compression, as Yomitan dictionaries can be. */
function zip(files: Record<string, string>): Uint8Array {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc32 = (data: Uint8Array) => {
    let c = 0xffffffff;
    for (const byte of data) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const parts: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  for (const [name, text] of Object.entries(files)) {
    const nameBytes = new TextEncoder().encode(name);
    const data = new TextEncoder().encode(text);
    const crc = crc32(data);
    const local = new DataView(new ArrayBuffer(30));
    local.setUint32(0, 0x04034b50, true);
    local.setUint16(4, 20, true);
    local.setUint32(14, crc, true);
    local.setUint32(18, data.length, true);
    local.setUint32(22, data.length, true);
    local.setUint16(26, nameBytes.length, true);
    parts.push(new Uint8Array(local.buffer), nameBytes, data);
    const entry = new DataView(new ArrayBuffer(46));
    entry.setUint32(0, 0x02014b50, true);
    entry.setUint16(4, 20, true);
    entry.setUint16(6, 20, true);
    entry.setUint32(16, crc, true);
    entry.setUint32(20, data.length, true);
    entry.setUint32(24, data.length, true);
    entry.setUint16(28, nameBytes.length, true);
    entry.setUint32(42, offset, true);
    central.push(new Uint8Array(entry.buffer), nameBytes);
    offset += 30 + nameBytes.length + data.length;
  }
  const centralSize = central.reduce((n, part) => n + part.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, Object.keys(files).length, true);
  end.setUint16(10, Object.keys(files).length, true);
  end.setUint32(12, centralSize, true);
  end.setUint32(16, offset, true);
  return new Uint8Array(
    [...parts, ...central, new Uint8Array(end.buffer)].flatMap((p) => [...p]),
  );
}

const init = spawnSync(
  bin,
  ["host", "init", "--user", user, "--user-id", user, "--bind", `127.0.0.1:${port}`, "--password-stdin"],
  { env, input: password + "\n", encoding: "utf8" },
);
if (init.status !== 0) throw new Error(init.stderr);
writeFileSync(
  join(root, "host.toml"),
  `bind = "127.0.0.1:${port}"\nfrontend_dir = ${JSON.stringify(dist)}\n`,
);
const files = join(root, "cache/users", user, "files");
mkdirSync(files, { recursive: true });

const ruby = (base: string, reading: string) =>
  `<ruby>${base}<rt>${reading}</rt></ruby>`;
const htmlPage = (lang: string, body: string) =>
  `<!DOCTYPE html><html lang="${lang}"><head><meta charset="utf-8"><title>card</title></head><body><p>${body}</p></body></html>`;
const cards = [
  {
    language: "ja",
    front: `私は${ruby("日本語", "にほんご")}を${ruby("勉強", "べんきょう")}しています。昨日は寿司を食べました。<mark>_____</mark> means "to eat".`,
    back: `私は${ruby("日本語", "にほんご")}を${ruby("勉強", "べんきょう")}しています。昨日は寿司を食べました。<mark>${ruby("食", "た")}べる</mark> means "to eat".`,
  },
  {
    language: "zh",
    front: `我在学习${ruby("中文", "zhōngwén")}。<mark>_____</mark> means "hello".`,
    back: `我在学习${ruby("中文", "zhōngwén")}。<mark>${ruby("你好", "nǐ hǎo")}</mark> means "hello".`,
  },
];
const dictionaries = [
  zip({
    "index.json": JSON.stringify({ title: "E2E Japanese", revision: "1", sourceLanguage: "ja", attribution: "CC BY-SA" }),
    "term_bank_1.json": JSON.stringify([
      ["食べる", "たべる", "v1", "v1", 10, ["to eat"], 1, ""],
      ["勉強", "べんきょう", "n vs", "vs", 10, ["study"], 2, ""],
      ["勉強する", "べんきょうする", "vs", "vs", 12, ["to study"], 3, ""],
      ["日本語", "にほんご", "n", "", 10, ["Japanese"], 4, ""],
      ["日本", "にほん", "n", "", 9, ["Japan"], 5, ""],
    ]),
  }),
  zip({
    "index.json": JSON.stringify({ title: "E2E Chinese", revision: "1", sourceLanguage: "zh" }),
    "term_bank_1.json": JSON.stringify([["中文", "zhōng wén", "", "", 0, ["Chinese language"], 1, ""]]),
  }),
];

function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

/** The viewport point of the first `text` in a shown card, once it is there. */
async function textPoint(page: Page, text: string): Promise<{ x: number; y: number }> {
  const handle = await page.waitForFunction(
    (text) => {
      for (const iframe of document.querySelectorAll("iframe")) {
        const doc = iframe.contentDocument;
        if (!doc?.body || iframe.getBoundingClientRect().height === 0) continue;
        const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
        let node;
        while ((node = walker.nextNode())) {
          const i = (node as Text).data.indexOf(text);
          if (i < 0 || node.parentElement?.closest("rt")) continue;
          const range = doc.createRange();
          range.setStart(node, i);
          range.setEnd(node, i + 1);
          const r = range.getBoundingClientRect();
          const f = iframe.getBoundingClientRect();
          if (r.width === 0) continue;
          return { x: f.left + r.left + r.width / 2, y: f.top + r.top + r.height / 2 };
        }
      }
      return null;
    },
    text,
    { timeout: 10_000 },
  );
  return (await handle.jsonValue())!;
}

/** Reviews the card of the `n`th note of `cards`. */
async function review(page: Page, n: number) {
  await page.goto(`${origin}/review?query=${encodeURIComponent(`tag=card-${n}`)}&start`);
  await page.waitForFunction(() =>
    [...document.querySelectorAll("iframe")].some((f) => f.contentDocument?.body?.textContent?.trim()),
  );
}

/**
 * Looks up the word at `text` on the card: K on the word under the pointer, or where the browser
 * doesn't let the app listen to the card (WebKit), a click with Look up words on.
 */
async function lookUp(page: Page, text: string) {
  const point = () =>
    textPoint(page, text).catch(async (e) => {
      await shot(page, "failed");
      throw e;
    });
  if (engineName === "webkit") {
    const toggle = page.getByRole("button", { name: "Look up words" });
    if ((await toggle.getAttribute("aria-pressed")) !== "true") await toggle.click();
    // Once the card's iframe lets clicks through to the app, and where the toggle moved the card
    await page.waitForFunction(() =>
      [...document.querySelectorAll("iframe")].some((f) => f.style.pointerEvents === "none"),
    );
    const { x, y } = await point();
    await page.mouse.click(x, y);
    return;
  }
  // The lookup listens once it has loaded, which can be after the card shows: a pointer that
  // hasn't moved since isn't known, so this moves it again, to where the word is now, until K
  // finds it
  for (let attempt = 0; attempt < 10; attempt++) {
    const { x, y } = await point();
    await page.mouse.move(x - 1 - (attempt % 2), y);
    await page.mouse.move(x, y);
    await page.keyboard.press("K");
    if (await page.waitForSelector(".language-tools-popup", { timeout: 500 }).then(() => true, () => false))
      return;
  }
}

async function popupText(page: Page) {
  await page.waitForSelector(".language-tools-popup", { timeout: 10_000 }).catch(async (e) => {
    await shot(page, "no-popup");
    throw e;
  });
  await page.waitForSelector(".language-tools-popup li", { timeout: 10_000 }).catch(async () => {
    await shot(page, "no-entries");
    throw new Error(`No entries: ${await page.locator(".language-tools-popup").innerText()}`);
  });
  return page.locator(".language-tools-popup").innerText();
}

async function shot(page: Page, name: string) {
  if (shots) await page.screenshot({ path: join(shots, `${engineName}-${name}.png`) });
}

const engines = { chromium, firefox, webkit };
const engineName = process.argv[2] ?? "chromium";
if (!Object.hasOwn(engines, engineName)) throw new Error(`Unknown browser: ${engineName}`);
if (shots) mkdirSync(shots, { recursive: true });

const server = spawn(bin, [], { env, stdio: ["ignore", "ignore", "inherit"] });
let browser;
try {
  for (let i = 0; i < 100; i++) {
    if (server.exitCode !== null) throw new Error("The disposable host exited");
    if (await fetch(origin).then((r) => r.ok, () => false)) break;
    await Bun.sleep(100);
  }
  // The notes, through the API, and their cards' HTML as pandoc writes it
  const auth = await fetch(`${origin}/api/auth/token`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ user, password, name: "e2e" }),
  }).then((r) => r.json());
  const api = (path: string, body: unknown) =>
    fetch(`${origin}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${auth.token}` },
      body: JSON.stringify(body),
    }).then(async (r) => {
      if (!r.ok) throw new Error(`${path} failed (${r.status}): ${await r.text()}`);
      return r.json();
    });
  const parser = await api("/api/parsers", { name: "markdown-language-tools" });
  for (const [i, card] of cards.entries()) {
    const { notes } = await api("/api/notes", {
      parser_id: parser.id,
      requests: [{ data: "{{x}}", keywords: [], tags: [`card-${i + 1}`], is_suspended: false, custom_data: { language: card.language } }],
    });
    const name = formatNoteId(notes[0].id);
    writeFileSync(join(files, `${name}-1-front.html`), htmlPage(card.language, card.front));
    writeFileSync(join(files, `${name}-1-back.html`), htmlPage(card.language, card.back));
    writeFileSync(join(files, `${name}.html`), htmlPage(card.language, card.back));
  }
  browser = await engines[engineName as keyof typeof engines].launch();
  const newContext = async (options: Parameters<typeof browser.newContext>[0] = {}) => {
    const context = await browser!.newContext({ serviceWorkers: "block", viewport: { width: 1000, height: 760 }, ...options });
    const login = await context.request.post(`${origin}/api/auth/login`, { data: { user, password } });
    check(login.ok(), `Login failed (${login.status()})`);
    const { csrf_token, user: account } = await login.json();
    for (const dictionary of dictionaries) {
      const imported = await context.request.post(`${origin}/api/language-tools/dictionaries`, {
        headers: { "X-CSRF-Token": csrf_token },
        data: Buffer.from(dictionary),
      });
      // The second context finds them imported already
      if (!imported.ok() && imported.status() !== 400)
        throw new Error(`Import failed (${imported.status()}): ${await imported.text()}`);
    }
    await context.addInitScript((credentials) => {
      localStorage.setItem("spares_credentials", JSON.stringify(credentials));
    }, { csrfToken: csrf_token, accountId: account.id, user, schedulerName: "fsrs" });
    return context;
  };

  const context = await newContext();
  const page = await context.newPage();
  page.on("pageerror", (e) => console.error(`pageerror: ${e.message}`));
  await review(page, 1);
  await shot(page, "1-front");

  // The word under the pointer, deinflected
  await lookUp(page, "勉");
  let text = await popupText(page);
  check(text.includes("勉強する") && text.includes("progressive"), `Expected 勉強する: ${text}`);
  await shot(page, "2-lookup");
  // j picks the next entry, Esc closes
  await page.keyboard.press("j");
  await page.waitForFunction(() => document.querySelector('.language-tools-popup [data-entry="1"] button'));
  await page.keyboard.press("Escape");
  await page.waitForSelector(".language-tools-popup", { state: "detached" });
  // K on the selection, in every browser
  await page.evaluate(() => {
    const iframe = [...document.querySelectorAll("iframe")].find((f) => f.getBoundingClientRect().height > 0)!;
    const doc = iframe.contentDocument!;
    const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
    let node;
    while ((node = walker.nextNode())) {
      const i = (node as Text).data.indexOf("寿司");
      if (i < 0) continue;
      const range = doc.createRange();
      range.setStart(node, i);
      range.setEnd(node, i + 2);
      doc.getSelection()!.removeAllRanges();
      doc.getSelection()!.addRange(range);
      return;
    }
  });
  await page.keyboard.press("K");
  await page.waitForSelector(".language-tools-popup");
  await page.waitForFunction(() => document.querySelector(".language-tools-popup")?.textContent?.includes("No entries"));
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    for (const iframe of document.querySelectorAll("iframe")) iframe.contentDocument?.getSelection()?.removeAllRanges();
  });

  // On the back, a reading looks up its base
  await page.keyboard.press("Space");
  await page.waitForTimeout(500);
  await lookUp(page, "食べま");
  text = await popupText(page);
  check(text.includes("食べる") && text.includes("polite past"), `Expected 食べる: ${text}`);
  await shot(page, "3-back-lookup");
  // Enter adds a card through quick capture, with this parser and the word
  await page.keyboard.press("Enter");
  await page.waitForSelector("dialog.quick-capture");
  await page.waitForFunction(() =>
    document.querySelector<HTMLSelectElement>("dialog.quick-capture select")?.selectedOptions[0]?.textContent?.includes("markdown-language-tools"),
  );
  await page.waitForFunction(() => document.querySelector("dialog.quick-capture")?.textContent?.includes("たべる"));
  await shot(page, "4-add-card");
  await page.close();

  // Chinese, in the dictionaries of the note's language only
  const zhPage = await context.newPage();
  await review(zhPage, 2);
  await lookUp(zhPage, "中");
  text = await popupText(zhPage);
  check(text.includes("Chinese language") && !text.includes("E2E Japanese"), `Expected 中文: ${text}`);
  await shot(zhPage, "5-chinese");

  // Taps look words up with the toggle, where the browser emulates a touchscreen
  if (engineName !== "firefox") {
    const phone = await newContext({ viewport: { width: 390, height: 780 }, hasTouch: true, isMobile: true });
    const phonePage = await phone.newPage();
    await review(phonePage, 1);
    await phonePage.getByRole("button", { name: "Look up words" }).tap();
    const point = await textPoint(phonePage, "日本語");
    await phonePage.touchscreen.tap(point.x, point.y);
    text = await popupText(phonePage);
    check(text.includes("Japanese"), `Expected 日本語: ${text}`);
    await shot(phonePage, "6-phone");
  }
  console.log(`${engineName}: language tools passed`);
} finally {
  await browser?.close();
  server.kill();
  rmSync(root, { recursive: true, force: true });
}
