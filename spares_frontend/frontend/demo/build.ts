// Builds the demo collection and writes it next to a demo build of the web app, which loads it
// into the browser's copy instead of downloading it from a host (see src/local/demo.ts):
//
//   VITE_SPARES_DEMO=1 bunx vite build --base /spares/ --outDir dist-demo
//   bun demo/build.ts --out dist-demo
//
// or `bun run build-demo`. It makes a host of its own in a temporary root (`--root` keeps it),
// creates the notes in notes.ts through the API (uploading their images and audio), makes up
// three months of reviews for them (with days off, crams, a forgotten card and leeches),
// reschedules every card from those reviews, does what a user would have done today (burying,
// postponing, planning a break, splitting and merging notes), renders the notes, and writes to
// `<out>/demo/`:
//
// - `collection.sqlite`: the database, which `load_snapshot` in spares_wasm restores;
// - `config.json`: the host's `DeviceConfig` (`GET /api/sync/config`);
// - `bundle.bin`: the rendered files devices download (`/api/sync/files/changed`) and the files
//   Typst cards read, one after another, and `image-occlusions/` the image occlusions' files;
// - `manifest.json`: what is there (and where in `bundle.bin`), and when it was built.
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
import { formatNoteId } from "../src/utils";
import { finishDemoBuild } from "./finish";
import {
  LATER_NOTES,
  LEECH_EASE,
  NOTES,
  OCCLUSIONS,
  SAVED_SEARCHES,
  type DemoNote,
  type DemoOcclusion,
  type ParserName,
} from "./notes";

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
// Renders get far longer than a server's (60 seconds, 120 of CPU): on a CI runner, LuaTeX's first
// renders of the Japanese and Chinese notes build its font caches, and a render stopped for
// taking too long fails the whole build
writeFileSync(
  join(ROOT, "host.toml"),
  `bind = "127.0.0.1:${PORT}"\n\n[render]\ntimeout_seconds = 900\ncpu_seconds = 900\n`,
);
const COLLECTION = join(ROOT, "users", USER);
const CONFIG = join(COLLECTION, "config/config.toml");

/** The date `days` from now, as config.toml writes dates */
const dateIn = (days: number) => new Date((NOW + days * DAY) * 1000).toISOString().slice(0, 10);

/**
 * The host's config.toml: Typst cards compiled by browsers only, as on a server, and the
 * collection's settings that differ from the defaults, which the host moves into the collection
 * when it opens it (see spares_core::settings): lighter Sundays, new cards held back until the
 * notes they link to are learned, a preset for languages, and two goals: one on track, and one
 * with a milestone missed.
 */
function writeConfig(): void {
  mkdirSync(dirname(CONFIG), { recursive: true });
  writeFileSync(
    CONFIG,
    `render_browser_parsers = false

[sync]
replica = ${HOST_REPLICA}

[easy_days.days_to_workload_percentage]
Sun = 0.5

[queue]
new_after_linked_notes = true

[queue.presets.languages]
tags = ["languages"]
new_cards_daily_limit = 5
learning_steps = [60, 600, 3600]
bury_new_siblings = true

[goals.linear-algebra-exam]
date = "${dateIn(45)}"
retention = 0.9
query = 'tag="math:linear-algebra"'

[goals.algorithms-interview]
date = "${dateIn(21)}"
learned_by = "${dateIn(7)}"
query = 'tag="cs:algorithms"'
milestones = [{ date = ${dateIn(-2)}, new_left = 0 }]

[goals.algorithms-interview.parts]
graphs = 'keyword=BFS or keyword=Dijkstra or keyword="Bellman-Ford" or keyword="Topological sort" or keyword="Union-find"'
data-structures = 'keyword="Hash table" or keyword=Heap or keyword=Trie or keyword="Stacks and queues" or keyword="Hash table resizing"'
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
  forgotten?: number;
  buried?: boolean;
}

const parsers = {} as Record<ParserName, number>;
/** Notes by their first keyword, for `@card(…)` in notes.ts */
const noteByKeyword = new Map<string, number>();

/** A multipart POST to the API, returning its JSON */
async function upload<T>(path: string, form: FormData): Promise<T> {
  const res = await fetch(`${ORIGIN}${path}`, {
    method: "POST",
    headers: { Authorization: `Bearer ${TOKEN}` },
    body: form,
  });
  if (!res.ok) throw new Error(`POST ${path}: ${res.status} ${await res.text()}`);
  return (await res.json()) as T;
}

/** A file in this directory, uploaded as `kind` ("image" or "audio"), as its snippet */
async function uploadMedia(kind: "image" | "audio", name: string): Promise<string> {
  const form = new FormData();
  form.append("parser_id", String(parsers.markdown));
  form.append(kind, new File([readFileSync(join(HERE, name))], name));
  return (await upload<{ snippet: string }>(kind === "image" ? "/api/images" : "/api/audio", form)).snippet;
}

/** `data` with notes.ts's placeholders (`@image(…)`, `@audio(…)`, `@card(…)`) filled in */
async function fillIn(data: string): Promise<string> {
  for (const [placeholder, kind, name] of data.matchAll(/@(image|audio)\(([^)]+)\)/g))
    data = data.replace(placeholder, await uploadMedia(kind as "image" | "audio", name));
  return data.replace(/@card\(([^/)]+)\/(\d+)\)/g, (_, keyword: string, order: string) => {
    const id = noteByKeyword.get(keyword);
    if (id === undefined) throw new Error(`@card(${keyword}/${order}): no note has the first keyword ${keyword}`);
    return `${formatNoteId(id)}/${order}`;
  });
}

/** Creates `note` through the API, and returns its id */
async function create(note: DemoNote): Promise<number> {
  const data = await fillIn(note.data);
  const { notes } = await api<{ notes: { id: number; card_count: number }[] }>("POST", "/api/notes", {
    parser_id: parsers[note.parser],
    requests: [
      {
        data,
        keywords: note.keywords ?? [],
        tags: note.tags,
        is_suspended: note.suspended ?? false,
        custom_data: note.customData ?? {},
      },
    ],
  }).catch((e: Error) => {
    throw new Error(`The demo note "${data.split("\n")[0]}" was refused: ${e.message}`);
  });
  if (notes[0].card_count === 0)
    throw new Error(`A demo note made no cards, so its syntax is wrong:\n${data}`);
  if (note.keywords?.[0]) noteByKeyword.set(note.keywords[0], notes[0].id);
  return notes[0].id;
}

/** Creates an image occlusion note: its image and clozes SVG, as the editor saves them, and the note */
async function createOcclusion(occlusion: DemoOcclusion): Promise<number> {
  const masks = occlusion.masks
    .map(
      ({ x, y, width, height, settings }, i) =>
        `<rect id="svg_${i + 1}" x="${x}" y="${y}" width="${width}" height="${height}" fill="#FFEBA2"` +
        (settings ? ` data-cloze-settings="${settings}"` : "") +
        "/>",
    )
    .join("");
  const clozes =
    `<svg width="${occlusion.width}" height="${occlusion.height}" xmlns="http://www.w3.org/2000/svg">` +
    `<g class="layer" id="markup-group"><title>Markup</title>${occlusion.markup ?? ""}</g>` +
    `<g class="layer" id="clozes-group"><title>Clozes</title>${masks}</g></svg>`;
  // Named after the note, as a file someone uploads would be
  const name = `${occlusion.keywords[0].toLowerCase().replace(/\W+/g, "-")}.png`;
  const form = new FormData();
  form.append("parser_id", String(parsers.markdown));
  form.append("image", new File([readFileSync(join(HERE, occlusion.image))], name, { type: "image/png" }));
  form.append("clozes", clozes);
  const { snippet } = await upload<{ snippet: string }>("/api/image-occlusions", form);
  return create({
    parser: "markdown",
    data: occlusion.data.replace("@occlusion", snippet),
    tags: occlusion.tags,
    keywords: occlusion.keywords,
    ago: occlusion.ago,
  });
}

async function createNotes(): Promise<CreatedNote[]> {
  for (const name of ["markdown", "typst", "latex-note", "markdown-language-tools"] as ParserName[])
    parsers[name] = (await api<{ id: number }>("POST", "/api/parsers", { name })).id;
  const created: CreatedNote[] = [];
  // Oldest first, so that ids follow creation, as they would have. The sort is stable, so notes
  // created on the same day keep their order in notes.ts.
  const ordered = [
    ...NOTES.map((note) => ({ ago: note.ago, note })),
    ...OCCLUSIONS.map((occlusion) => ({ ago: occlusion.ago, occlusion })),
  ].sort((a, b) => b.ago - a.ago);
  for (const item of ordered) {
    if ("occlusion" in item) {
      const { occlusion } = item;
      created.push({ id: await createOcclusion(occlusion), ago: occlusion.ago, ease: occlusion.ease, suspended: false });
      continue;
    }
    const { note } = item;
    created.push({
      id: await create(note),
      ago: note.ago,
      ease: note.ease ?? 0.75,
      suspended: note.suspended ?? false,
      forgotten: note.forgotten,
      buried: note.buried,
    });
  }
  return created;
}

// Made-up reviews

/** A `review_log` row (`ReviewLogKind`): a review, a forget, or a cram in a filtered tag */
interface Review {
  cardId: number;
  at: number;
  kind: number;
  /** `null` for a forget */
  rating: number | null;
  previousState: number;
  /** `null` for a forget or a cram */
  scheduled: number | null;
  /** Seconds to recall and to rate, `null` for a forget */
  recall: number | null;
  rate: number | null;
  /** A cram's filtered tag */
  tagId?: number;
}

const KIND = { review: 0, forget: 1, cram: 16 };
const RATING = { again: 1, hard: 2, good: 3, easy: 4 };
const STATE = { new: 0, learning: 1, review: 2, relearning: 3 };
/** `[leech] lapses_threshold` in the default config.toml */
const LEECH_LAPSES = 8;

// Days are study days (starting at `day_starts_at`, 4 AM), in UTC, as reviews are at UTC times.
const DAY_STARTS_AT = 4 * 3600;
const studyDay = (t: number) => Math.floor((t - DAY_STARTS_AT) / DAY);
const TODAY = studyDay(NOW);
/** The evening of the night before the linear algebra quiz, crammed in a filtered tag */
const CRAM_AGO = 13;
/**
 * Days without reviews, as anyone has: a few days here and there, and a holiday of four days.
 * Reviews due on them wait for the next day.
 */
const DAYS_OFF = new Set<number>();
for (let i = 0; i < 7; i++) DAYS_OFF.add(TODAY - (2 + Math.floor(rand() * 85)));
for (let i = 0; i < 4; i++) DAYS_OFF.add(TODAY - 52 + i);
DAYS_OFF.delete(TODAY - CRAM_AGO);

/** `at`, or the same time on the next day that isn't one of `DAYS_OFF` */
function onDayOn(at: number): number {
  while (DAYS_OFF.has(studyDay(at))) at += DAY;
  return at;
}

/** 8 AM or 7 PM UTC, and up to two and a half hours after, on the day of `t` */
const atTimeOfDay = (t: number) =>
  t - (t % DAY) + (rand() < 0.6 ? 8 : 19) * 3600 + Math.floor(rand() * 2.5 * 3600);
/** 8 PM UTC, `ago` days ago */
const eveningOf = (ago: number) => {
  const t = NOW - ago * DAY;
  return t - (t % DAY) + 20 * 3600;
};

/** Seconds to recall a card rated `rating`: failing takes longest, and easy cards are quick */
function recallSeconds(rating: number, ease: number): number {
  const [low, high] =
    rating === RATING.again ? [8, 25] : rating === RATING.hard ? [6, 16] : rating === RATING.good ? [2, 10] : [1, 4];
  return Math.round(low + rand() * (high - low) * (1.4 - ease * 0.6));
}

/**
 * Reviews of a card first seen `start` (a time), before `until`, by someone who knows it as well
 * as `ease` says. Below `LEECH_EASE`, it lapses about every other review.
 */
function reviewsOf(cardId: number, start: number, ease: number, until = NOW): Review[] {
  const reviews: Review[] = [];
  const review = (at: number, rating: number, previousState: number, scheduled: number) =>
    reviews.push({
      cardId,
      at,
      kind: KIND.review,
      rating,
      previousState,
      scheduled,
      recall: recallSeconds(rating, ease),
      rate: 1 + Math.floor(rand() * 2),
    });
  let at = onDayOn(atTimeOfDay(start));
  if (at >= until) return reviews;
  // Learning: Again a few times for a hard card, then Good
  let state = STATE.new;
  while (rand() > 0.35 + ease * 0.6) {
    review(at, RATING.again, state, 600);
    state = STATE.learning;
    at += 600 + Math.floor(rand() * 300);
  }
  let interval = 1;
  review(at, RATING.good, state, interval * DAY);
  state = STATE.review;
  const lapse = ease < LEECH_EASE ? 0.5 : (1 - ease) * 0.4;
  for (;;) {
    at = onDayOn(atTimeOfDay(at + interval * DAY + Math.floor((rand() - 0.3) * interval * 0.25) * DAY));
    if (at >= until) break;
    const roll = rand();
    let rating: number;
    if (roll < lapse) rating = RATING.again;
    else if (roll < lapse + 0.12) rating = RATING.hard;
    else if (roll > 1 - 0.12 * ease) rating = RATING.easy;
    else rating = RATING.good;
    if (rating === RATING.again) {
      review(at, rating, state, 600);
      at += 600 + Math.floor(rand() * 600);
      interval = 1;
      review(at, RATING.good, STATE.relearning, DAY);
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
    review(at, rating, state, interval * DAY);
  }
  return reviews;
}

/** The lapses in `reviews`: Again on a card that had graduated */
const lapsesOf = (reviews: Review[]) =>
  reviews.filter((r) => r.kind === KIND.review && r.rating === RATING.again && r.previousState === STATE.review).length;

interface Backdated {
  reviews: number;
  crams: number;
  /** Notes with a card that lapsed `LEECH_LAPSES` times */
  leeches: number[];
}

/**
 * Dates the notes and cards from when they were "created", and gives them reviews since: a
 * forget and the reviews after it for notes `forgotten`, and crams of the cards of `cramTag`'s
 * query on the evening `CRAM_AGO` days ago.
 */
function backdate(created: CreatedNote[], cramTag: { id: number; noteIds: Set<number> }): Backdated {
  const db = new Database(join(COLLECTION, "collection.sqlite"));
  const reviews: Review[] = [];
  const leeches: number[] = [];
  db.transaction(() => {
    for (const note of created) {
      const createdAt = NOW - note.ago * DAY - Math.floor(rand() * 4 * 3600);
      db.query("UPDATE note SET created_at = ?1, updated_at = ?1 WHERE id = ?2").run(createdAt, note.id);
      db.query("UPDATE card SET created_at = ?1, updated_at = ?1 WHERE note_id = ?2").run(createdAt, note.id);
      if (note.suspended || note.ago === 0) continue;
      const cards = db.query("SELECT id FROM card WHERE note_id = ? ORDER BY \"order\"").all(note.id) as { id: number }[];
      let leech = false;
      for (const [i, card] of cards.entries()) {
        // Siblings start a few days apart, as their notes' cards were unburied
        const start = NOW - Math.max(1, note.ago - (i % 3)) * DAY;
        let ofCard: Review[];
        if (note.forgotten === undefined) {
          ofCard = reviewsOf(card.id, start, note.ease);
        } else {
          // Reset to new in the evening, and learned again from the next day
          const forgotAt = eveningOf(note.forgotten);
          const before = reviewsOf(card.id, start, note.ease, forgotAt);
          const forget: Review = {
            cardId: card.id,
            at: forgotAt,
            kind: KIND.forget,
            rating: null,
            previousState: before.length ? STATE.review : STATE.new,
            scheduled: null,
            recall: null,
            rate: null,
          };
          ofCard = [...before, forget, ...reviewsOf(card.id, forgotAt + DAY, note.ease)];
        }
        if (lapsesOf(ofCard) >= LEECH_LAPSES) leech = true;
        reviews.push(...ofCard);
      }
      if (leech) leeches.push(note.id);
    }
    // The cram: once through every card already learned by then, a second time for those failed
    const cramStart = eveningOf(CRAM_AGO);
    const learnedBy = new Set(reviews.filter((r) => r.kind === KIND.review && r.at < cramStart).map((r) => r.cardId));
    let at = cramStart;
    for (const note of created) {
      if (!cramTag.noteIds.has(note.id)) continue;
      const cards = db.query("SELECT id FROM card WHERE note_id = ? ORDER BY \"order\"").all(note.id) as { id: number }[];
      for (const card of cards) {
        if (!learnedBy.has(card.id)) continue;
        const ratings = rand() < 0.2 ? [RATING.again, RATING.good] : [rand() < 0.15 ? RATING.easy : RATING.good];
        for (const rating of ratings) {
          at += 20 + Math.floor(rand() * 40);
          reviews.push({
            cardId: card.id,
            at,
            kind: KIND.cram,
            rating,
            previousState: STATE.review,
            scheduled: null,
            recall: recallSeconds(rating, note.ease),
            rate: 1,
            tagId: cramTag.id,
          });
        }
      }
    }
    reviews.sort((a, b) => a.at - b.at);
    const insert = db.query(
      `INSERT INTO review_log (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
         recall_duration, rate_duration, previous_state, tag_id, custom_data, usn, previous_interval)
       VALUES (?, ?, ?, ?, ?, 'fsrs', ?, ?, ?, ?, ?, '{}', -1, NULL)`,
    );
    const first = ((db.query("SELECT COALESCE(MAX(id), 0) AS id FROM review_log").get() as { id: number }).id);
    let id = first - (first % ID_STEP) + HOST_REPLICA + ID_STEP;
    for (const r of reviews) {
      insert.run(id, r.cardId, r.at, r.kind, r.rating, r.scheduled, r.recall, r.rate, r.previousState, r.tagId ?? null);
      id += ID_STEP;
    }
    // Statistics recompute these from the reviews
    db.exec("DELETE FROM study_day_cache; DELETE FROM study_day_dirty; DELETE FROM study_day;");
  })();
  db.close();
  const crams = reviews.filter((r) => r.kind === KIND.cram).length;
  return { reviews: reviews.length - crams, crams, leeches };
}

// The bundle

interface RenderedFile {
  path: string;
  mtime: number;
  size: number;
  bucket: string;
}

/** Where a file is in `bundle.bin`: bytes `start` to `end`, and its `Content-Type` */
interface BundleEntry {
  start: number;
  end: number;
  type: string;
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

  // The rendered files and assets in one file, `bundle.bin`, which the browser loads with one
  // request rather than hundreds, each a round trip to a static host
  const parts: Uint8Array[] = [];
  let size = 0;
  const pack = (path: string, type: string): BundleEntry => {
    const bytes = readFileSync(path);
    parts.push(bytes);
    size += bytes.length;
    return { start: size - bytes.length, end: size, type };
  };
  const filesDir = join(ROOT, "cache/users", USER, "files");
  const bundledFiles = files
    .map((file) => file.path)
    .sort()
    .map((path) => ({ path, ...pack(join(filesDir, path), Bun.file(path).type) }));
  // Each asset's stored copy, named by its hash in the collection's directory. Typst reads them
  // as bytes, whatever their type.
  const stored = join(COLLECTION, "render_assets");
  const bundledAssets = assets.map((asset) => ({
    ...asset,
    ...pack(join(stored, asset.hash), "application/octet-stream"),
  }));
  writeFileSync(join(OUT, "bundle.bin"), Buffer.concat(parts));
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
      files: bundledFiles,
      assets: bundledAssets,
      imageOcclusions,
    }),
  );
}

// Run

/** The filtered tag crammed the night before a quiz, which doesn't reschedule */
const CRAM_TAG = "linear-algebra-quiz";

/** Ids of notes tagged `tag`, or one of its child tags */
function notesTagged(tag: string): Set<number> {
  const db = new Database(join(COLLECTION, "collection.sqlite"), { readonly: true });
  const rows = db
    .query("SELECT nt.note_id AS id FROM note_tag nt JOIN tag t ON t.id = nt.tag_id WHERE t.name = ?1 OR t.name LIKE ?1 || ':%'")
    .all(tag) as { id: number }[];
  db.close();
  return new Set(rows.map((row) => row.id));
}

/** Ids of the cards of `noteId`, soonest due first */
function cardsOf(noteId: number): number[] {
  const db = new Database(join(COLLECTION, "collection.sqlite"), { readonly: true });
  const rows = db.query("SELECT id FROM card WHERE note_id = ? ORDER BY due, \"order\"").all(noteId) as { id: number }[];
  db.close();
  return rows.map((row) => row.id);
}

/**
 * What someone would have done today: tag the leeches (rating does it, but these reviews were
 * made up), bury the card they keep putting off, postpone a few Spanish cards, plan a break, and
 * split and merge notes (LATER_NOTES).
 */
async function today(created: CreatedNote[], leeches: number[]): Promise<void> {
  if (leeches.length === 0) throw new Error("No note became a leech: lower an `ease` in notes.ts");
  cli(["note", "edit", "--ids", ...leeches.map(formatNoteId), "--tags-to-add", "leech"]);
  // Looking at the queue unburies yesterday's cards and records that today's unburying is done
  // (`last_unburied`), as opening the Review page would. Without it, the first look in the browser
  // would unbury the card buried below.
  cli(["card", "queue"]);
  for (const note of created.filter((note) => note.buried)) {
    // The tag `[queue.tag_buried]` gives a card buried on two days without a rating between
    const [card] = cardsOf(note.id);
    cli(["card", "edit", "--ids", String(card), "--tags-to-add", "buried"]);
    cli(["card", "edit", "--ids", String(card), "--special-state", "buried"]);
  }
  cli(["card", "postpone", "3", "-q", 'tag="languages:spanish" c.due<=today+3d']);
  // Reviewed ahead, so that a card of the plant cell is due, for the README's screenshot of an
  // image occlusion (screenshots.ts)
  cli(["card", "advance", "1", "-q", 'keyword="Plant cell"']);
  cli(["card", "break", "--from", dateIn(24), "--days", "5", "--spread", "4", "--yes"]);
  for (const note of LATER_NOTES) {
    await create(note);
    const merged = note.deletes?.map((keyword) => formatNoteId(noteByKeyword.get(keyword)!)) ?? [];
    if (merged.length) cli(["note", "delete", "--ids", ...merged]);
  }
}

writeConfig();
await startServer();
const created = await createNotes();
for (const search of SAVED_SEARCHES) await api("PUT", "/api/saved-searches", search);
cli(["tag", "add", "-n", CRAM_TAG, "-q", 'tag="math:linear-algebra"', "--reschedule", "false",
  "-d", "Crammed the night before the quiz, without changing when cards are due"]);
const cramTagId = (await api<{ id: number }>("GET", `/api/tags/name/${CRAM_TAG}`)).id;
await stopServer();
const { reviews: reviewCount, crams, leeches } = backdate(created, {
  id: cramTagId,
  noteIds: notesTagged("math:linear-algebra"),
});
await startServer();
// Memory states and due dates from the reviews
cli(["card", "reschedule", "--yes"]);
await today(created, leeches);
cli(["note", "generate", "--render", "--yes"]);
await writeBundle();
finishDemoBuild(args.out);
const size = Number(spawnSync("du", ["-sk", OUT], { encoding: "utf8" }).stdout.split("\t")[0]);
console.log(
  `Demo collection: ${created.length + LATER_NOTES.length - LATER_NOTES.flatMap((note) => note.deletes ?? []).length} notes, ` +
    `${reviewCount} reviews and ${crams} crams, ` +
    `${leeches.length} leeches, ${(size / 1024).toFixed(1)} MB in ${OUT}` +
    (args.root ? `\nIts host is in ${ROOT}` : ""),
);
if (!args.root) rmSync(ROOT, { recursive: true, force: true });
