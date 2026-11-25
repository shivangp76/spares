// Compare rating flows on a disposable real host. Run after building the frontend and server.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database } from "bun:sqlite";
import { chromium, firefox, webkit } from "playwright-core";
import { parseNoteId } from "../src/utils";

const root = mkdtempSync(join(tmpdir(), "spares-review-transition-"));
const bin =
  process.env.SPARES_SERVER_BIN ??
  join(import.meta.dir, "../../../target/debug/spares_server");
const dist =
  process.env.SPARES_FRONTEND_DIST ?? join(import.meta.dir, "../dist");
const port = Number(process.env.SPARES_E2E_PORT ?? 18797);
const origin = `http://127.0.0.1:${port}`;
const env = { ...process.env, SPARES_ROOT: root };
const password = "benchmark-password";
const user = "bench";
const init = spawnSync(
  bin,
  [
    "host",
    "init",
    "--user",
    user,
    "--user-id",
    user,
    "--bind",
    `127.0.0.1:${port}`,
    "--password-stdin",
  ],
  { env, input: password + "\n", encoding: "utf8" },
);
if (init.status !== 0) throw new Error(init.stderr);
writeFileSync(
  join(root, "host.toml"),
  `bind = "127.0.0.1:${port}"\nfrontend_dir = ${JSON.stringify(dist)}\n`,
);
const userDir = join(root, "users", user);
mkdirSync(join(userDir, "config"), { recursive: true });
writeFileSync(
  join(userDir, "config/config.toml"),
  "new_cards_daily_limit = 100\n",
);
const files = join(root, "cache/users", user, "files");
mkdirSync(files, { recursive: true });
const db = new Database(join(userDir, "collection.sqlite"));
db.exec("BEGIN; INSERT INTO parser (id,name) VALUES (1,'markdown')");
for (let i = 0; i < 40; i++) {
  const name =
    "a" + String.fromCharCode(97 + Math.floor(i / 26), 97 + (i % 26));
  const id = parseNoteId(name)!;
  db.query(
    "INSERT INTO note(id,data,custom_data,parser_id) VALUES (?,?,'{}',1)",
  ).run(id, `Prompt ${i} {{answer}}`);
  db.query(
    `INSERT INTO card(id,note_id,"order",back_type,due,stability,difficulty,desired_retention,state,custom_data) VALUES (?,?,1,2,0,0,0,0.9,0,'{}')`,
  ).run(i + 1, id);
  for (const side of ["front", "back"])
    writeFileSync(join(files, `${name}-1-${side}.html`), `<p>${side}-${i}</p>`);
}
db.exec("COMMIT");
db.close();

const server = spawn(bin, [], { env, stdio: ["ignore", "ignore", "inherit"] });
let browser;
try {
  for (let i = 0; i < 100; i++) {
    if (server.exitCode !== null) throw new Error("The disposable host exited");
    if (
      await fetch(origin).then(
        (r) => r.ok,
        () => false,
      )
    )
      break;
    await Bun.sleep(100);
  }
  const engines = { chromium, firefox, webkit };
  const engine = process.argv[2] ?? "chromium";
  if (!Object.hasOwn(engines, engine)) throw new Error(`Unknown browser: ${engine}`);
  browser = await engines[engine as keyof typeof engines].launch();
  const context = await browser.newContext({ serviceWorkers: "block" });
  const login = await context.request.post(origin + "/api/auth/login", {
    data: { user, password },
  });
  if (!login.ok()) throw new Error(`Login failed (${login.status()})`);
  const { csrf_token, user: account } = await login.json();
  // A large front stays a file download, even when the client requests inline HTML.
  async function select() {
    const response = await context.request.post(origin + "/api/review", {
      headers: { "X-CSRF-Token": csrf_token },
      data: { filter: { Query: "c.id=40" }, include_front_html: true },
    });
    if (!response.ok()) throw new Error(`Selection failed (${response.status()})`);
    return response.json();
  }
  const small = await select();
  if (small?.front_html !== "<p>front-39</p>")
    throw new Error("Missing inline front");
  const path = join(files, small.card_front_web_path);
  writeFileSync(path, "x".repeat(64 * 1024 + 1));
  const large = await select();
  if (
    large?.card_front_web_path !== small.card_front_web_path ||
    large.front_html !== undefined
  )
    throw new Error("A large front did not fall back to its file");
  writeFileSync(path, small.front_html);
  await context.addInitScript(
    (credentials) => {
      localStorage.setItem("spares_credentials", JSON.stringify(credentials));
    },
    {
      csrfToken: csrf_token,
      accountId: account.id,
      user,
      schedulerName: "fsrs",
    },
  );
  const page = await context.newPage();
  const modes = ["separate", "combined", "inline"] as const;
  let mode: (typeof modes)[number] = "inline";
  let submissions = 0;
  let selections = 0;
  let frontFiles = 0;
  let delay = 40;
  await page.route(/\/files\/.*-front\.html$/, async (route) => {
    frontFiles++;
    if (delay) await Bun.sleep(delay);
    await route.continue();
  });
  // Each critical request incurs the same controlled delay in all three flows.
  await page.route(/\/api\/review(?:\/submit)?$/, async (route) => {
    const request = route.request();
    if (delay) await Bun.sleep(delay);
    if (request.url().endsWith("/submit")) {
      submissions++;
      const body = request.postDataJSON();
      if (!body.next_review?.include_front_html)
        throw new Error("The rating did not ask for its rendered successor");
      if (mode === "separate") delete body.next_review;
      else if (mode === "combined") delete body.next_review.include_front_html;
      await route.continue({ postData: JSON.stringify(body) });
    } else {
      selections++;
      const body = request.postDataJSON();
      if (mode === "separate") delete body.include_front_html;
      await route.continue({ postData: JSON.stringify(body) });
    }
  });
  await page.goto(origin + "/review");
  await page.getByRole("button", { name: "Start Review" }).click();
  for (let i = 0; i < 30; i++) {
    if (i === 24) delay = 0;
    mode = delay ? modes[i % modes.length] : "inline";
    await page.getByRole("button", { name: /Show Answer/ }).click();
    await page.getByRole("button", { name: /Good/i }).waitFor();
    const before = selections;
    const filesBefore = frontFiles;
    const elapsed = await page.evaluate(async () => {
      const front = (visibleOnly = false) =>
        [...document.querySelectorAll("iframe")]
          .filter(
            (f) => !visibleOnly || f.checkVisibility({ visibilityProperty: true }),
          )
          .map((f) => f.contentDocument?.body?.textContent?.trim())
          .find((text) => text?.startsWith("front-"));
      const old = front();
      if (!old) throw new Error("Missing current front");
      const start = performance.now();
      [...document.querySelectorAll("button")]
        .find((b) => /Good/i.test(b.textContent ?? ""))!
        .click();
      let sawLoading = false;
      for (;;) {
        await new Promise(requestAnimationFrame);
        if (performance.now() - start > 10000)
          throw new Error("Next front did not render");
        const loading = document.querySelector(
          '.skeleton[role="status"]:not(.loading-delayed)',
        );
        if (loading?.checkVisibility({ visibilityProperty: true })) {
          if (front(true) || document.querySelector(".review-answer-bar"))
            throw new Error("Card controls or content appeared during loading");
          sawLoading = true;
          continue;
        }
        const text = front(true);
        if (!text || text === old || !sawLoading)
          throw new Error("The loading page did not cover the transition");
        return performance.now() - start;
      }
    });
    if (
      submissions !== i + 1 ||
      selections - before !== Number(mode === "separate") ||
      frontFiles - filesBefore !== Number(mode !== "inline")
    )
      throw new Error("Unexpected request count");
    console.log(`${mode} (${delay} ms/request) ${elapsed.toFixed(2)} ms`);
  }

  // Undo during submission must restore the rated back before another rating can run.
  const answer = page.getByRole("button", { name: /Show Answer/ });
  const good = page.getByRole("button", { name: /Good/i });
  await answer.click();
  const shownSide = page.frameLocator("iframe:visible").locator("body");
  const back = await shownSide.innerText();
  delay = 100;
  await good.click();
  await page.keyboard.press("u");
  await good.waitFor();
  if (await shownSide.innerText() !== back)
    throw new Error("Undo during loading restored the wrong card");
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null && server.signalCode === null)
    await new Promise((resolve) => server.once("exit", resolve));
  rmSync(root, { recursive: true, force: true });
}
