// Serves a demo build (see build.ts) as GitHub Pages does: its files under `base`, and its
// `404.html`, the app's page, for every other path, so the app's routes load. For screenshots.ts
// and e2e/demo.ts, and to try a build locally:
//
//   bun demo/serve.ts dist-demo        # http://127.0.0.1:8798/spares/
import { existsSync, statSync } from "node:fs";
import { join, normalize, resolve } from "node:path";

export function serveDemo(dir: string, port: number, base = "/spares/"): { origin: string; stop: () => void } {
  const root = resolve(dir);
  if (!existsSync(join(root, "demo/manifest.json")))
    throw new Error(`${root} isn't a demo build: run \`bun run build-demo\` first`);
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port,
    fetch(request) {
      const { pathname } = new URL(request.url);
      if (!pathname.startsWith(base)) return new Response("Not found", { status: 404 });
      const relative = normalize(decodeURIComponent(pathname.slice(base.length)));
      if (relative.startsWith("..")) return new Response("Not found", { status: 404 });
      let path = join(root, relative);
      if (existsSync(path) && statSync(path).isDirectory()) path = join(path, "index.html");
      if (existsSync(path)) return new Response(Bun.file(path));
      return new Response(Bun.file(join(root, "404.html")), {
        status: 404,
        headers: { "Content-Type": "text/html" },
      });
    },
  });
  return { origin: `http://127.0.0.1:${server.port}`, stop: () => server.stop(true) };
}

if (import.meta.main) {
  const dir = process.argv[2] ?? "dist-demo";
  const { origin } = serveDemo(dir, Number(process.env.PORT ?? 8798));
  console.log(`Serving ${dir} at ${origin}/spares/`);
}
