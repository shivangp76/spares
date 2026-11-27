// The demo build (src/demo.ts) in headless Chromium, WebKit and Firefox, from static files with no
// host: it loads its collection, reviews a card, keeps the review across a reload, and starts over
// with Reset.
//
//   bun run build-demo && bun e2e/demo.ts [chromium|webkit|firefox …]
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, firefox, webkit, type Page } from "playwright-core";
import { serveDemo } from "../demo/serve";

const ENGINES = { chromium, webkit, firefox };
const TIMEOUT = 60_000;
const engines = (process.argv.length > 2 ? process.argv.slice(2) : Object.keys(ENGINES)) as (keyof typeof ENGINES)[];
for (const engine of engines) if (!(engine in ENGINES)) throw new Error(`Unknown browser ${engine}`);

const { origin, stop } = serveDemo(
  process.env.SPARES_DEMO_DIST ?? join(import.meta.dir, "../dist-demo"),
  Number(process.env.SPARES_E2E_PORT ?? 8795),
);
const APP = `${origin}/spares`;

/** The number in the review page's "N reviews (M crams) in total", once it shows */
async function totalReviews(page: Page): Promise<number> {
  // Pointing at a day of the heatmap, as the mouse may be after a click, shows that day's instead
  await page.mouse.move(0, 0);
  const text = await page
    .getByText(/[\d,]+ reviews?( \([\d,]+ crams?\))? in total/)
    .first()
    .textContent();
  return Number(text!.match(/[\d,]+/)![0].replaceAll(",", ""));
}

async function openReview(page: Page): Promise<void> {
  await page.goto(`${APP}/review`);
  await page.locator(".demo-banner").waitFor();
  await page.getByRole("button", { name: "Start Review" }).waitFor();
}

async function run(engine: keyof typeof ENGINES): Promise<void> {
  console.log(engine);
  const profile = mkdtempSync(join(tmpdir(), `spares-demo-e2e-${engine}-`));
  // A persistent context, since WebKit has no OPFS in an ephemeral one
  const context = await ENGINES[engine].launchPersistentContext(profile, { headless: true });
  context.setDefaultTimeout(TIMEOUT);
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  try {
    const started = Date.now();
    await openReview(page);
    const before = await totalReviews(page);
    console.log(`  loaded ${before} reviews in ${Date.now() - started} ms`);
    if (before < 500) throw new Error(`The demo has only ${before} reviews`);

    await page.getByRole("button", { name: "Start Review" }).click();
    // A button rather than Space, which a card with a typed answer would type into
    await page.getByRole("button", { name: /Show Answer|Check Answer/ }).click();
    await page.getByRole("button", { name: /Good/ }).click();
    await page.getByText(/Reviewed 1\b/).waitFor();
    console.log("  reviewed a card");

    // Kept in the browser's copy
    await openReview(page);
    const after = await totalReviews(page);
    if (after !== before + 1) throw new Error(`After a review and a reload, ${after} reviews, not ${before + 1}`);
    console.log("  kept it across a reload");

    // Something only a host does says so, rather than failing on a request to nowhere
    await page.goto(`${APP}/statistics`);
    await page.getByRole("button", { name: /Optimize parameters/ }).click();
    await page.getByText(/isn't part of this demo/).first().waitFor();
    console.log("  said what needs a host");

    await openReview(page);
    await page.locator(".demo-banner").getByRole("button", { name: "Reset" }).click();
    await page.waitForEvent("load");
    await page.getByRole("button", { name: "Start Review" }).waitFor();
    const reset = await totalReviews(page);
    if (reset !== before) throw new Error(`After Reset, ${reset} reviews, not ${before}`);
    console.log("  started over with Reset");
    if (errors.length > 0) throw new Error(`Page errors:\n${errors.join("\n")}`);
  } catch (e) {
    const shot = join(tmpdir(), `spares-demo-e2e-${engine}.png`);
    await page.screenshot({ path: shot }).catch(() => {});
    console.error(`  page when it failed: ${shot}${errors.length ? `\n  page errors: ${errors.join("; ")}` : ""}`);
    throw e;
  } finally {
    await context.close();
    rmSync(profile, { recursive: true, force: true });
  }
}

let failed = false;
for (const engine of engines) {
  try {
    await run(engine);
  } catch (e) {
    failed = true;
    console.error(`  FAILED: ${e instanceof Error ? e.message : e}`);
  }
}
stop();
process.exit(failed ? 1 : 0);
