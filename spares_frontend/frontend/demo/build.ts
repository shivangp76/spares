// Builds the demo collection and writes it next to a demo build of the web app, which loads it
// into the browser's copy instead of downloading it from a host (see src/local/demo.ts):
//
//   VITE_SPARES_DEMO=1 bunx vite build --base /spares/ --outDir dist-demo
//   bun demo/build.ts --out dist-demo
//
// or `bun run build-demo`. It makes a host of its own in a temporary root (`--root` keeps it),
// creates the notes in notes.ts through the API, makes up three months of reviews for them,
// reschedules every card from those reviews, renders the notes, and writes to `<out>/demo/`:
//
// - `collection.sqlite`: the database, which `load_snapshot` in spares_wasm restores;
// - `config.json`: the host's `DeviceConfig` (`GET /api/sync/config`);
// - `files/`: the rendered files devices download (`/api/sync/files/changed`), `assets/` the
//   files Typst cards read, by hash, and `image-occlusions/` the image occlusions' files;
// - `manifest.json`: what is there, and when it was built.
//
// Reviews are dated from when it runs, so the demo is rebuilt now and then (see
// .github/workflows/demo.yml) to keep cards due. It needs `spares` and `spares_server` (by
// default from ../../target/debug, else `SPARES_BIN` and `SPARES_SERVER_BIN`), and pandoc and
// LaTeX (latexmk, and lualatex with luatexja for the Japanese and Chinese notes) to render
// Markdown and LaTeX notes.
import { Database } from "bun:sqlite";
import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { finishDemoBuild } from "./finish";
import { CELL_LABELS, NOTES, SAVED_SEARCHES, type ParserName } from "./notes";

const HERE = dirname(new URL(import.meta.url).pathname);
const REPO = resolve(HERE, "../../..");
const SPARES_BIN = process.env.SPARES_BIN ?? join(REPO, "target/debug/spares");
const SERVER_BIN =
  process.env.SPARES_SERVER_BIN ?? join(REPO, "target/debug/spares_server");
const USER = "demo";
const PORT = Number(process.env.SPARES_DEMO_PORT ?? 8797);
const ORIGIN = `http://127.0.0.1:${PORT}`;
const DAY = 24 * 60 * 60;
/** The replica the demo host is: the laptop, whose note ids are the shortest (see `spares_core::ids`) */
const HOST_REPLICA = 0;
/** `spares_core::ids::ID_STEP` */
const ID_STEP = 8;

const { values: args } = parseArgs({
  options: {
    out: { type: "string" },
    root: { type: "string" },
  },
});
if (!args.out) throw new Error("usage: bun demo/build.ts --out <web app build> [--root <dir>]");
const OUT = resolve(args.out, "demo");
const ROOT = args.root ? resolve(args.root) : mkdtempSync(join(tmpdir(), "spares-demo-"));
const NOW = Math.floor(Date.now() / 1000);

// Settings of the user's own spares, such as its host's root, which this host must not touch
const env: Record<string, string> = {};
for (const [key, value] of Object.entries(process.env))
  if (!key.startsWith("SPARES_") && value !== undefined) env[key] = value;
Object.assign(env, { SPARES_ROOT: ROOT, RUST_LOG: process.env.RUST_LOG ?? "warn" });

/** A pseudo-random number generator with a fixed seed, so builds on the same day agree */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rand = random(20261126);

function run(bin: string, argv: string[], input?: string): string {
  const result = spawnSync(bin, argv, { env, input, encoding: "utf8" });
  if (result.status !== 0)
    throw new Error(`${bin} ${argv.join(" ")} failed:\n${result.stderr}${result.stdout}`);
  return result.stdout;
}

// The host

if (existsSync(join(ROOT, "host.toml")))
  throw new Error(`${ROOT} already has a host: give an empty or new --root`);
mkdirSync(ROOT, { recursive: true });
const TOKEN = run(
  SERVER_BIN,
  ["host", "init", "--user", USER, "--user-id", USER, "--bind", `127.0.0.1:${PORT}`, "--password-stdin", "--print-token"],
  "demo-password\n",
).trim();
writeFileSync(join(ROOT, "host.toml"), `bind = "127.0.0.1:${PORT}"\n`);
const COLLECTION = join(ROOT, "users", USER);
const CONFIG = join(COLLECTION, "config/config.toml");

/** The host's default config.toml, with Typst cards compiled by browsers only, as on a server, and a goal */
function writeConfig(): void {
  const goalDate = new Date((NOW + 45 * DAY) * 1000).toISOString().slice(0, 10);
  const config = readFileSync(CONFIG, "utf8")
    .replace(/^render_browser_parsers = true$/m, "render_browser_parsers = false")
    .replace(/^\[sync\]$/m, `[sync]\nreplica = ${HOST_REPLICA}`);
  if (!config.includes("render_browser_parsers = false") || !config.includes("replica = "))
    throw new Error("config.toml has no `render_browser_parsers` or `[sync]` to change");
  writeFileSync(
    CONFIG,
    `${config}
[goals.linear-algebra-exam]
date = "${goalDate}"
retention = 0.9
query = 'tag="math:linear-algebra"'
`,
  );
}

let server: ReturnType<typeof spawn> | null = null;

async function startServer(): Promise<void> {
  server = spawn(SERVER_BIN, [], { env, stdio: ["ignore", "inherit", "inherit"] });
  for (let i = 0; ; i++) {
    try {
      if ((await fetch(`${ORIGIN}/api/healthcheck`)).ok) return;
    } catch {
      // Not listening yet
    }
    if (i > 150) throw new Error("the demo host didn't start");
    await Bun.sleep(100);
  }
}

async function stopServer(): Promise<void> {
  if (!server) return;
  const stopped = new Promise((done) => server!.once("exit", done));
  server.kill("SIGTERM");
  await stopped;
  server = null;
}

process.on("exit", () => server?.kill("SIGKILL"));

async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${ORIGIN}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${TOKEN}`,
      "Content-Type": "application/json",
      // Sync requests say which version of the protocol they speak (`spares_core::sync::protocol`)
      "X-Spares-Sync-Protocol": "3",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
  const text = await res.text();
  return (text ? JSON.parse(text) : null) as T;
}

/** The CLI, against this host */
function cli(argv: string[]): string {
  return run(SPARES_BIN, ["--host", ORIGIN, ...argv]);
}
env.SPARES_TOKEN = TOKEN;

// The notes

interface CreatedNote {
  id: number;
  ago: number;
  ease: number;
  suspended: boolean;
}

async function createNotes(): Promise<CreatedNote[]> {
  const parsers: Record<ParserName, number> = {} as Record<ParserName, number>;
  for (const name of ["markdown", "typst", "latex-note", "markdown-language-tools"] as ParserName[])
    parsers[name] = (await api<{ id: number }>("POST", "/api/parsers", { name })).id;
  const created: CreatedNote[] = [];
  const create = async (
    parser: ParserName,
    note: {
      data: string;
      tags: string[];
      keywords?: string[];
      suspended?: boolean;
      customData?: Record<string, unknown>;
    },
  ) => {
    const { notes } = await api<{ notes: { id: number; card_count: number }[] }>("POST", "/api/notes", {
      parser_id: parsers[parser],
      requests: [
        {
          data: note.data,
          keywords: note.keywords ?? [],
          tags: note.tags,
          is_suspended: note.suspended ?? false,
          custom_data: note.customData ?? {},
        },
      ],
    }).catch((e: Error) => {
      throw new Error(`The demo note "${note.data.split("\n")[0]}" was refused: ${e.message}`);
    });
    if (notes[0].card_count === 0)
      throw new Error(`A demo note made no cards, so its syntax is wrong:\n${note.data}`);
    return notes[0].id;
  };
  // Oldest first, so that ids follow creation, as they would have
  const ordered = [...NOTES].sort((a, b) => b.ago - a.ago);
  const occlusionAgo = 38;
  let occlusionMade = false;
  for (const note of ordered) {
    if (!occlusionMade && note.ago < occlusionAgo) {
      created.push({ id: await createOcclusionNote(parsers.markdown), ago: occlusionAgo, ease: 0.7, suspended: false });
      occlusionMade = true;
    }
    const id = await create(note.parser, note);
    created.push({ id, ago: note.ago, ease: note.ease ?? 0.75, suspended: note.suspended ?? false });
  }
  return created;

  async function createOcclusionNote(parserId: number): Promise<number> {
    const masks = CELL_LABELS.map(
      ({ x, y, width, height }, i) =>
        `<rect id="svg_${i + 1}" x="${x}" y="${y}" width="${width}" height="${height}" fill="#FFEBA2"/>`,
    ).join("");
    const clozes = `<svg width="600" height="500" xmlns="http://www.w3.org/2000/svg"><g class="layer" id="markup-group"><title>Markup</title></g><g class="layer" id="clozes-group"><title>Clozes</title>${masks}</g></svg>`;
    const form = new FormData();
    form.append("parser_id", String(parserId));
    form.append("image", new File([readFileSync(join(HERE, "cell.png"))], "plant-cell.png", { type: "image/png" }));
    form.append("clozes", clozes);
    const res = await fetch(`${ORIGIN}/api/image-occlusions`, {
      method: "POST",
      headers: { Authorization: `Bearer ${TOKEN}` },
      body: form,
    });
    if (!res.ok) throw new Error(`POST /api/image-occlusions: ${res.status} ${await res.text()}`);
    const { snippet } = (await res.json()) as { snippet: string };
    return create("markdown", {
      data: `# Parts of a plant cell\n\nName each labelled part.\n\n${snippet}`,
      tags: ["science:biology"],
      keywords: ["Plant cell", "Organelles"],
    });
  }
}

// Made-up reviews

interface Review {
  cardId: number;
  at: number;
  rating: number;
  previousState: number;
  scheduled: number;
  recall: number;
}

const RATING = { again: 1, hard: 2, good: 3, easy: 4 };
const STATE = { new: 0, learning: 1, review: 2, relearning: 3 };
/** Days without reviews, by days before the build, as anyone has: reviews due then wait a day */
const DAYS_OFF = new Set(
  Array.from({ length: 9 }, () => 2 + Math.floor(rand() * 85)),
);

/** `at`, or the next day that isn't one of `DAYS_OFF` */
function onDayOn(at: number): number {
  while (DAYS_OFF.has(Math.floor((NOW - at) / DAY))) at += DAY;
  return at;
}

/** Reviews of a card first seen `ago` days ago, by someone who knows it as well as `ease` says */
function reviewsOf(cardId: number, ago: number, ease: number): Review[] {
  const reviews: Review[] = [];
  // Mornings and evenings
  const timeOfDay = () => (rand() < 0.6 ? 8 : 19) * 3600 + Math.floor(rand() * 2.5 * 3600);
  const startOfDay = (t: number) => t - (t % DAY);
  let at = onDayOn(startOfDay(NOW - ago * DAY) + timeOfDay());
  if (at >= NOW) return reviews;
  const recall = () => 3 + Math.floor(rand() * 12);
  // Learning: Again a few times for a hard card, then Good
  let state = STATE.new;
  while (rand() > 0.35 + ease * 0.6) {
    reviews.push({ cardId, at, rating: RATING.again, previousState: state, scheduled: 600, recall: recall() });
    state = STATE.learning;
    at += 600 + Math.floor(rand() * 300);
  }
  let interval = 1;
  reviews.push({ cardId, at, rating: RATING.good, previousState: state, scheduled: interval * DAY, recall: recall() });
  state = STATE.review;
  for (;;) {
    at = onDayOn(
      startOfDay(at + interval * DAY + Math.floor((rand() - 0.3) * interval * 0.25) * DAY) + timeOfDay(),
    );
    if (at >= NOW) break;
    const roll = rand();
    let rating: number;
    if (roll < (1 - ease) * 0.4) rating = RATING.again;
    else if (roll < (1 - ease) * 0.4 + 0.12) rating = RATING.hard;
    else if (roll > 1 - 0.12 * ease) rating = RATING.easy;
    else rating = RATING.good;
    if (rating === RATING.again) {
      reviews.push({ cardId, at, rating, previousState: state, scheduled: 600, recall: recall() });
      at += 600 + Math.floor(rand() * 600);
      interval = 1;
      reviews.push({ cardId, at, rating: RATING.good, previousState: STATE.relearning, scheduled: DAY, recall: recall() });
      state = STATE.review;
      continue;
    }
    interval = Math.max(
      1,
      Math.round(
        interval *
          (rating === RATING.hard ? 1.2 : rating === RATING.good ? 1.6 + ease * 1.4 : 3 + ease * 1.5),
      ),
    );
    reviews.push({ cardId, at, rating, previousState: state, scheduled: interval * DAY, recall: recall() });
  }
  return reviews;
}

/** Dates the notes and cards from when they were "created", and gives them reviews since. */
function backdate(created: CreatedNote[]): number {
  const db = new Database(join(COLLECTION, "collection.sqlite"));
  const reviews: Review[] = [];
  db.transaction(() => {
    for (const note of created) {
      const createdAt = NOW - note.ago * DAY - Math.floor(rand() * 4 * 3600);
      db.query("UPDATE note SET created_at = ?1, updated_at = ?1 WHERE id = ?2").run(createdAt, note.id);
      db.query("UPDATE card SET created_at = ?1, updated_at = ?1 WHERE note_id = ?2").run(createdAt, note.id);
      if (note.suspended || note.ago === 0) continue;
      const cards = db.query("SELECT id FROM card WHERE note_id = ? ORDER BY \"order\"").all(note.id) as { id: number }[];
      for (const [i, card] of cards.entries()) {
        // Siblings start a few days apart, as their notes' cards were unburied
        reviews.push(...reviewsOf(card.id, Math.max(1, note.ago - (i % 3)), note.ease));
      }
    }
    reviews.sort((a, b) => a.at - b.at);
    const insert = db.query(
      `INSERT INTO review_log (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
         recall_duration, rate_duration, previous_state, tag_id, custom_data, usn, previous_interval)
       VALUES (?, ?, ?, 0, ?, 'fsrs', ?, ?, ?, ?, NULL, '{}', -1, NULL)`,
    );
    const first = ((db.query("SELECT COALESCE(MAX(id), 0) AS id FROM review_log").get() as { id: number }).id);
    let id = first - (first % ID_STEP) + HOST_REPLICA + ID_STEP;
    for (const review of reviews) {
      insert.run(id, review.cardId, review.at, review.rating, review.scheduled, review.recall, 1 + Math.floor(rand() * 2), review.previousState);
      id += ID_STEP;
    }
    // Statistics recompute these from the reviews
    db.exec("DELETE FROM study_day_cache; DELETE FROM study_day_dirty; DELETE FROM study_day;");
  })();
  db.close();
  return reviews.length;
}

// The bundle

interface RenderedFile {
  path: string;
  mtime: number;
  size: number;
  bucket: string;
}

async function writeBundle(): Promise<void> {
  const config = await api<unknown>("GET", "/api/sync/config");
  const listed = (await api<{ files: RenderedFile[] }>("POST", "/api/sync/files/changed", { buckets: {} })).files;
  // The web app shows a Markdown note or card with its HTML file, and only falls back to the PDF
  // without one, so the PDFs that have one are left out: they are most of the size
  const html = new Set(listed.filter((file) => file.path.endsWith(".html") && file.size > 0).map((file) => file.path));
  const files = listed.filter((file) => !(file.path.endsWith(".pdf") && html.has(file.path.replace(/\.pdf$/, ".html"))));
  await stopServer();
  rmSync(OUT, { recursive: true, force: true });
  mkdirSync(OUT, { recursive: true });

  const db = new Database(join(COLLECTION, "collection.sqlite"));
  db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  db.query("VACUUM INTO ?").run(join(OUT, "collection.sqlite"));
  const assets = db.query("SELECT path, hash FROM render_asset ORDER BY path").all() as { path: string; hash: string }[];
  db.close();
  writeFileSync(join(OUT, "config.json"), JSON.stringify(config));

  const filesDir = join(ROOT, "cache/users", USER, "files");
  for (const file of files) {
    const target = join(OUT, "files", file.path);
    mkdirSync(dirname(target), { recursive: true });
    copyFileSync(join(filesDir, file.path), target);
  }
  // Each asset's stored copy, named by its hash in the collection's directory
  const stored = join(COLLECTION, "render_assets");
  mkdirSync(join(OUT, "assets"), { recursive: true });
  for (const asset of assets) copyFileSync(join(stored, asset.hash), join(OUT, "assets", asset.hash));
  const occlusionsDir = join(COLLECTION, "media/image_occlusions");
  const imageOcclusions = existsSync(occlusionsDir) ? readdirSync(occlusionsDir) : [];
  mkdirSync(join(OUT, "image-occlusions"), { recursive: true });
  for (const name of imageOcclusions)
    copyFileSync(join(occlusionsDir, name), join(OUT, "image-occlusions", name));

  writeFileSync(
    join(OUT, "manifest.json"),
    JSON.stringify({
      built: NOW,
      replica: 2,
      files: files.map((file) => file.path).sort(),
      assets,
      imageOcclusions,
    }),
  );
}

// Run

writeConfig();
await startServer();
const created = await createNotes();
for (const search of SAVED_SEARCHES) await api("PUT", "/api/saved-searches", search);
await stopServer();
const reviewCount = backdate(created);
await startServer();
// Memory states and due dates from the reviews
cli(["card", "reschedule", "--yes"]);
cli(["note", "generate", "--render", "--yes"]);
await writeBundle();
finishDemoBuild(args.out);
const size = Number(spawnSync("du", ["-sk", OUT], { encoding: "utf8" }).stdout.split("\t")[0]);
console.log(
  `Demo collection: ${created.length} notes, ${reviewCount} reviews, ${(size / 1024).toFixed(1)} MB in ${OUT}` +
    (args.root ? `\nIts host is in ${ROOT}` : ""),
);
if (!args.root) rmSync(ROOT, { recursive: true, force: true });
