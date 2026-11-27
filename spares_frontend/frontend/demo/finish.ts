// Readies a demo build of the web app for a static host such as GitHub Pages. build.ts runs it after
// writing the collection, and run.ts after rebuilding only the app beside a collection it kept.
import { copyFileSync, existsSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

export function finishDemoBuild(dir: string): void {
  const out = resolve(dir);
  // The app's page at every route it doesn't have a file for, and no Jekyll, which would leave out
  // files starting with `_`
  const page = join(out, "index.html");
  if (existsSync(page)) copyFileSync(page, join(out, "404.html"));
  writeFileSync(join(out, ".nojekyll"), "");
  // Compressed copies of the app's files, which a static host such as GitHub Pages doesn't serve:
  // it compresses files itself
  for (const file of readdirSync(out, { recursive: true }) as string[])
    if (/\.(br|gz)$/.test(file) && !file.startsWith("demo/")) rmSync(join(out, file));
}
