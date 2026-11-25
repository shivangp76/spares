// Exercise durable checkpoint recovery with a real IndexedDB, including a new JS context.
// Run with bun e2e/copy_checkpoint.ts from the frontend directory.
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { join } from "node:path";

const build = await Bun.build({
  entrypoints: [join(import.meta.dir, "../src/local/copyCheckpoint.ts")],
  target: "browser",
});
if (!build.success) throw new AggregateError(build.logs);
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: (request) =>
    new URL(request.url).pathname === "/checkpoint.js"
      ? new Response(build.outputs[0], {
          headers: { "Content-Type": "text/javascript" },
        })
      : new Response("<!doctype html>", {
          headers: { "Content-Type": "text/html" },
        }),
});
const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  await page.goto(server.url.href);
  await page.evaluate(async () => {
    const { openCheckpoints } = await import("/checkpoint.js");
    const db = await openCheckpoints();
    for (let i = 0; i < 2000; i++) {
      db.file({ path: `note-${i}.pdf`, bucket: "a", mtime: 1, size: 2 });
      if ((i + 1) % 100 === 0) await db.save();
    }
    // A worker has finished another response but the tab closes before that checkpoint commits.
    db.file({ path: "missing.pdf", bucket: "a", mtime: 1, size: 2 });
    db.bucket("a", "complete");
    db.close(false);
  });
  await page.reload();
  const partial = await page.evaluate(async () => {
    const { openCheckpoints } = await import("/checkpoint.js");
    const db = await openCheckpoints();
    const state = [
      Object.keys(db.listing.files).length,
      db.listing.buckets.a ?? null,
    ];
    db.file({ path: "missing.pdf", bucket: "a", mtime: 1, size: 2 });
    db.bucket("a", "complete");
    await db.save();
    db.close(true);
    return state;
  });
  assert.deepEqual(partial, [2000, null]);
  await page.reload();
  assert.deepEqual(
    await page.evaluate(async () => {
      const { openCheckpoints } = await import("/checkpoint.js");
      const db = await openCheckpoints();
      const state = [
        Object.keys(db.listing.files).length,
        db.listing.buckets.a,
      ];
      db.close(true);
      return state;
    }),
    [2001, "complete"],
  );
} finally {
  await browser.close();
  server.stop();
}
