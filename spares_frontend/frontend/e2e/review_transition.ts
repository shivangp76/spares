// Compare both rating flows on a disposable real host. Run after building the frontend and server.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Database } from "bun:sqlite";
import { chromium } from "playwright-core";
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
  browser = await chromium.launch();
  const context = await browser.newContext({ serviceWorkers: "block" });
  const login = await context.request.post(origin + "/api/auth/login", {
    data: { user, password },
  });
  if (!login.ok()) throw new Error(`Login failed (${login.status()})`);
  const { csrf_token, user: account } = await login.json();
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
  let combined = false;
  let submissions = 0;
  let selections = 0;
  // Both paths incur the same controlled delay for each critical API request.
  await page.route(/\/api\/review(?:\/submit)?$/, async (route) => {
    const request = route.request();
    await Bun.sleep(40);
    if (request.url().endsWith("/submit")) {
      submissions++;
      const body = request.postDataJSON();
      if (!body.next_review)
        throw new Error("The rating did not ask for its successor");
      if (!combined) delete body.next_review;
      await route.continue({ postData: JSON.stringify(body) });
    } else {
      selections++;
      await route.continue();
    }
  });
  await page.goto(origin + "/review");
  await page.getByRole("button", { name: "Start Review" }).click();
  for (let i = 0; i < 24; i++) {
    combined = i % 2 === 1;
    await page.getByRole("button", { name: /Show Answer/ }).click();
    await page.getByRole("button", { name: /Good/i }).waitFor();
    const before = selections;
    const elapsed = await page.evaluate(async () => {
      const front = () =>
        [...document.querySelectorAll("iframe")]
          .map((f) => f.contentDocument?.body?.textContent?.trim())
          .find((text) => text?.startsWith("front-"));
      const old = front();
      if (!old) throw new Error("Missing current front");
      const start = performance.now();
      [...document.querySelectorAll("button")]
        .find((b) => /Good/i.test(b.textContent ?? ""))!
        .click();
      for (;;) {
        await new Promise(requestAnimationFrame);
        const text = front();
        if (text && text !== old) return performance.now() - start;
        if (performance.now() - start > 10000)
          throw new Error("Next front did not render");
      }
    });
    if (submissions !== i + 1 || selections - before !== Number(!combined))
      throw new Error("Unexpected request count");
    console.log(
      `${combined ? "combined" : "separate"} ${elapsed.toFixed(2)} ms`,
    );
  }
} finally {
  await browser?.close();
  server.kill();
  if (server.exitCode === null && server.signalCode === null)
    await new Promise((resolve) => server.once("exit", resolve));
  rmSync(root, { recursive: true, force: true });
}
