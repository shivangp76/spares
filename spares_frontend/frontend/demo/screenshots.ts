// Takes the README's screenshots from a demo build (see build.ts), in headless Chromium. Run it
// from anywhere in the repository with `cargo xtask screenshots`, or here with
// `bun demo/screenshots.ts` (`bun run screenshots`), after a change to a page they show:
//
//   bun demo/screenshots.ts                # builds the demo as far as it needs (run.ts --build), then shoots
//   bun demo/screenshots.ts --fresh        # makes the demo collection again first
//   bun demo/screenshots.ts --no-build     # shoots the demo build already in dist-demo/
//   bun demo/screenshots.ts --only graph   # takes only that one
//
// Each is taken in light and dark (`<name>-light.webp`, `<name>-dark.webp`, with cwebp), which the
// README shows by the reader's colour scheme, with the demo's banner hidden. Then it checks the
// README against them: it fails if the README shows a screenshot it doesn't take, and names any
// file in docs/screenshots that the README doesn't show. Commit the README and docs/screenshots
// together. `--out` writes them elsewhere, and skips the check.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium, type BrowserContext, type Page } from "playwright-core";
import { serveDemo } from "./serve";

const REPO = resolve(import.meta.dir, "../../..");
const README = join(REPO, "README.md");
const SCREENSHOTS = join(REPO, "docs/screenshots");
const DEFAULT_DIST = join(import.meta.dir, "../dist-demo");

const { values: args } = parseArgs({
  options: {
    out: { type: "string", default: SCREENSHOTS },
    dist: { type: "string", default: DEFAULT_DIST },
    only: { type: "string" },
    fresh: { type: "boolean" },
    app: { type: "boolean" },
    "no-build": { type: "boolean" },
  },
});
const OUT = resolve(args.out!);

// run.ts builds only dist-demo/: another `--dist` is shot as it is
if (!args["no-build"] && resolve(args.dist!) === DEFAULT_DIST) {
  const flags = [...(args.fresh ? ["--fresh"] : []), ...(args.app ? ["--app"] : [])];
  const built = spawnSync(process.execPath, [join(import.meta.dir, "run.ts"), "--build", ...flags], { stdio: "inherit" });
  if (built.status !== 0) {
    console.error("screenshots: building the demo failed");
    process.exit(1);
  }
}

mkdirSync(OUT, { recursive: true });
const { origin, stop } = serveDemo(args.dist!, Number(process.env.PORT ?? 8799));
const APP = `${origin}/spares`;
const TIMEOUT = 60_000;

/** The demo, loaded in a fresh browser profile */
async function open(
  colorScheme: "light" | "dark",
  viewport: { width: number; height: number },
  mobile = false,
): Promise<{ context: BrowserContext; page: Page; close: () => Promise<void> }> {
  const profile = mkdtempSync(join(tmpdir(), "spares-screenshots-"));
  const context = await chromium.launchPersistentContext(profile, {
    headless: true,
    colorScheme,
    viewport,
    deviceScaleFactor: 2,
    isMobile: mobile,
    hasTouch: mobile,
  });
  context.setDefaultTimeout(TIMEOUT);
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log(`  [pageerror] ${e.message}`));
  await page.goto(`${APP}/review`);
  await page.locator(".navbar").waitFor();
  await page.addStyleTag({ content: ".demo-banner { display: none !important; }" });
  return {
    context,
    page,
    close: async () => {
      await context.close();
      rmSync(profile, { recursive: true, force: true });
    },
  };
}

/** Goes to `path` in the app, and waits for the page to settle: its fetches, renders and fonts */
async function go(page: Page, path: string): Promise<void> {
  await page.evaluate((path) => {
    window.history.pushState(null, "", path);
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, `/spares${path}`);
  await settle(page);
}

async function settle(page: Page): Promise<void> {
  // Typst may keep fetching fonts for a while
  await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => {});
  await page.evaluate(() => document.fonts.ready);
  // Skeletons and Typst compiles
  await page.waitForFunction(() => document.querySelectorAll(".skeleton").length === 0, null, { timeout: TIMEOUT }).catch(() => {});
  await page.waitForTimeout(1500);
}

/** A screenshot of the page as `<name>-<scheme>.webp`, which is a tenth of the PNG's size */
async function shoot(page: Page, name: string, scheme: string): Promise<void> {
  const png = join(OUT, `${name}-${scheme}.png`);
  const path = png.replace(/\.png$/, ".webp");
  await page.screenshot({ path: png });
  const webp = spawnSync("cwebp", ["-quiet", "-q", "88", png, "-o", path]);
  if (webp.status !== 0) throw new Error(`cwebp failed (brew install webp): ${webp.stderr}`);
  rmSync(png);
  console.log(`  ${path}`);
}

type Shot = { name: string; take: (page: Page) => Promise<void> };

/** Starts reviewing the cards `query` finds, and shows the first one's answer */
async function review(page: Page, query: string): Promise<void> {
  await go(page, "/review");
  await page.getByPlaceholder(/Filter cards due today/).fill(query);
  await page.getByRole("button", { name: "Start Review" }).click();
  await settle(page);
  await page.keyboard.press("Space");
  await settle(page);
}

const DESKTOP: Shot[] = [
  {
    name: "review",
    take: (page) => review(page, 'tag="math:calculus" parser_name=typst'),
  },
  {
    name: "notes",
    take: async (page) => {
      await go(page, "/notes?q=" + encodeURIComponent('tag="math:linear-algebra"'));
      await page.locator("tbody tr").nth(1).click();
      await settle(page);
    },
  },
  {
    name: "image-occlusion",
    take: (page) => review(page, 'keyword="Plant cell"'),
  },
  { name: "statistics", take: (page) => go(page, "/statistics") },
  {
    name: "graph",
    take: async (page) => {
      await go(page, "/graph");
      // Its explanation, open on a first visit, would cover the graph
      const help = page.locator("details", { has: page.locator("summary", { hasText: "What do these mean?" }) });
      if (await help.evaluate((details) => (details as HTMLDetailsElement).open)) {
        await help.locator("summary").click();
      }
      // Not `check()`: the page applies it after the click returns
      const hide = page.getByLabel("Hide unlinked notes");
      if (!(await hide.isChecked())) await hide.click();
      const fit = page.getByRole("button", { name: "Fit" });
      await fit.scrollIntoViewIfNeeded();
      await page.mouse.wheel(0, 300);
      await fit.click();
      await settle(page);
    },
  },
];

const SCHEMES = ["light", "dark"] as const;

/** Fails if the README shows a screenshot this script doesn't take, and names the files in
 * docs/screenshots it doesn't show, which are left from screenshots no longer taken */
function checkReadme(): void {
  const taken = new Set(DESKTOP.flatMap((shot) => SCHEMES.map((scheme) => `${shot.name}-${scheme}.webp`)));
  const shown = new Set([...readFileSync(README, "utf8").matchAll(/docs\/screenshots\/([\w.-]+)/g)].map((m) => m[1]));
  const missing = [...shown].filter((name) => !taken.has(name));
  const unshown = readdirSync(SCREENSHOTS).filter((name) => !shown.has(name));
  if (unshown.length > 0)
    console.log(`\nThe README doesn't show ${unshown.map((name) => `docs/screenshots/${name}`).join(", ")}: show or delete them`);
  if (missing.length > 0) {
    console.error(
      `\nThe README shows ${missing.map((name) => `docs/screenshots/${name}`).join(", ")}, which demo/screenshots.ts doesn't take: add them to DESKTOP, or take them out of the README`,
    );
    process.exitCode = 1;
  }
}

if (args.only && !DESKTOP.some((shot) => shot.name === args.only)) {
  stop();
  console.error(`screenshots: --only ${args.only}: there is no such screenshot (${DESKTOP.map((shot) => shot.name).join(", ")})`);
  process.exit(1);
}
for (const scheme of SCHEMES) {
  console.log(scheme);
  const { page, close } = await open(scheme, { width: 1280, height: 800 });
  for (const shot of DESKTOP) {
    if (args.only && shot.name !== args.only) continue;
    await shot.take(page);
    await shoot(page, shot.name, scheme);
  }
  await close();
}
stop();
if (OUT === SCREENSHOTS) checkReadme();
