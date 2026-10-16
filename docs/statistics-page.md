# Plan: Statistics page enhancements

Status: proposed (2026-10-14, updated 2026-10-16)

## Goal

Make the statistics answer the questions Anki's stats screen answers, in the web app and the CLI:
how much is coming up, how well am I remembering, how much am I studying, and what does my
collection look like. Every statistic can be narrowed to a search query (Anki's "deck/collection/search"
scope) and a period.

## Where things stand

- `spares_core/src/api/statistics.rs`
  - `get_statistics` returns today's figures: cards studied, recall and rate time, crams and cram
    time, card counts by state, due counts by state, `due_count_by_date`, and the
    advance/postpone-safe counts. The due counts by state are capped by what is left of the daily
    limits and the queue presets (`cap_counts`), so they are what is left to study today. It loads
    every card into memory (`SELECT * FROM card`).
  - `get_review_history` returns reviews and crams per local date. The review heatmap uses it, but
    the heatmap is drawn on `ReviewPage.tsx`, not on the stats page.
- `StatisticsResponse` and `ReviewHistoryResponse` are defined in `spares_core/src/schema.rs:874`.
  The server routes are `/api/review/statistics` and `/api/review/history`
  (`spares_server/src/route.rs:195`).
- `spares_frontend/frontend/src/pages/StatisticsPage.tsx` has five sections: Studied (with Crams
  and Cram time tiles), Due, Scheduling (advance/postpone forms), All cards by state, and Upcoming
  due (a table with inline bars). There's no chart library; `ReviewHeatmap.tsx` is hand-drawn SVG.
- `spares_cli/src/statistics.rs` prints the same sections as text (`format_statistics`), or JSON
  with `--json`. `spares_cli/src/history.rs` draws the heatmap in the terminal.
- Data available:
  - `review_log`: `kind`, `rating`, `previous_state`, `scheduled_time` (the interval the card was
    actually given), `recall_duration`, `rate_duration`, `reviewed_at`, `tag_id`.
  - `kind` is Review=0, Forget=1 or Cram=16 (`ReviewLogKind` in `model.rs`). Kinds from
    `ReviewLogKind::FIRST_UNREPLAYED` (16) up are recorded but not replayed. A cram has a rating
    and durations but no `scheduled_time`.
  - `card`: `due`, `stability`, `difficulty`, `state`, `special_state`, `created_at`.
  - `review_log` is indexed on `(card_id, reviewed_at)`, `reviewed_at`, `tag_id` and `usn`.
    `id` is the rowid, so the first index also orders by `id` within a timestamp.
  - `LAST_REVIEW` in `api/queue.rs` is the SQL for a card's last review time, crams excluded.
  - State ids are New=0, Learning=1, Review=2, Relearning=3. FSRS rating ids are Again=1 to
    Easy=4 (`schedulers/fsrs/utils.rs`).

## What to add

Each item below is a section of the page, with a matching CLI section and JSON field.

| # | Section | Contents | Source |
|---|---------|----------|--------|
| 1 | **Today** (extend) | Keep the existing tiles, crams included. Add % correct today (rating > Again), average seconds per card, and the split between learning and review. Add a cram % correct tile next to the Crams and Cram time tiles. | `review_log` |
| 2 | **Forecast** | Due cards per day for the next 7/30/90/365 days, stacked by learning vs review, plus a cumulative line. Overdue cards count as "today". New and suspended cards are left out; buried cards move to tomorrow (as `get_statistics` already does). Counts aren't capped by the daily limits, unlike `due_count_by_state`, so the forecast shows the real backlog. Replaces "Upcoming due". | `card` |
| 3 | **Reviews** | Reviews per day over the period, stacked by learning / relearning / young / mature / cram, with time spent per day. Show the total, the daily average, and the number of days studied. | `review_log` |
| 4 | **True retention** | A table like Anki's: rows for today, yesterday, last 7 days, last 30 days, last year and all time. Columns for young, mature and total. Each cell shows pass/fail counts and the retention %. Only reviews of cards in the Review state count. | `review_log` |
| 5 | **Answer buttons** | Count and % for each rating, grouped into learning / young / mature, with % correct per group. Rating labels come from the scheduler's ratings so other schedulers still work. | `review_log` |
| 6 | **Hourly breakdown** | Reviews and % correct for each local hour (0–23) over the period. Crams are counted separately and left out of the chart by default, with a toggle to show them. | `review_log` |
| 7 | **Card counts** | The existing per-state counts, plus suspended and buried, and young/mature split out of the Review state. | `card` |
| 8 | **Added** | Cards added per day over the period. | `card.created_at` |
| 9 | **Distributions** | Histograms of current interval, stability, difficulty and retrievability. Under retrievability, show average retrievability and estimated total knowledge (the sum of R across cards). | `card`, `LAST_REVIEW` |
| 10 | **Calendar** | Move or duplicate the heatmap from `ReviewPage` onto the stats page, and make it respect the query filter. | `get_review_history` |

**Definitions.** Put them in one place, `api/statistics.rs`, and document them in rustdoc on the
response structs (as `StatisticsResponse::cram_count` already is), not in `docs/src/concepts.md`.
- **Young/mature:** mature means the card's interval when it was reviewed was at least
  `[statistics] mature_threshold_days` (see below), 21 days by default as in Anki. For a review
  log row, that is the `scheduled_time` of the previous Review-kind row for the same card, found
  with a `LAG()` window over `(card_id ORDER BY reviewed_at, id)` restricted to `kind = Review`.
  For a card, it is `due - last_review`.
- **Pass:** rating is not the scheduler's lowest rating. Add `SrsScheduler::is_pass(rating)` so
  this isn't hard-coded to FSRS's id 1. FSRS's `lapses` and `get_leeches` still bind Again
  directly; they can switch to `is_pass` too.
- **Reviews:** only `kind = Review` rows count. `Forget` rows are excluded, as every current query
  already does. Orphaned rows (`card_id IS NULL`) count in totals but not in per-card
  distributions, which matches `get_review_history`.
- **Crams:** count in Today, Reviews, Hourly and the Calendar, shown apart from reviews as
  `get_statistics` and the heatmap already do. They are left out of true retention, answer
  buttons, young/mature and the last review time, since the scheduler did not learn from them.
- **% correct with crams:** crams get their own % correct and never go into the review one.
  Going twice through a 60-card tag adds 120 ratings, mostly Good on the second pass, which would
  swamp the review %. In Hourly they would also make cramming's late-evening hours look better
  than they are, so the chart leaves them out unless they are turned on. This differs from Anki,
  whose today line counts filtered-deck reviews in its correct %.

## Mature threshold setting

Add a `[statistics]` table to `config.toml`, as `StatisticsConfig` in `spares_core::config`,
with `#[serde(default)]` like `LeechConfig`, and a `statistics` field on `SparesExternalConfig`:

```toml
[statistics]
mature_threshold_days = 21
```

- `mature_threshold_days: u32`, in whole days because intervals are compared in days, and
  because seconds (as `set_card_due_date_duration` uses) would be hard to read here. Document it
  in rustdoc on the field, with an example, rather than on a separate page.
- `get_statistics_graphs` reads it with `read_external_config`, as `get_statistics` reads the
  limits. It applies everywhere young/mature is used: Reviews, True retention, Answer buttons
  and Card counts.
- The response returns the threshold it used, as `mature_threshold_days`, so the web app and CLI
  can label columns "Mature (≥ 21 days)" without reading the config themselves.
- It changes only statistics, not scheduling, so changing it needs no reschedule. All past
  reviews are reclassified the next time the page loads.

## API design

Add a single new endpoint and leave the existing ones unchanged, so older clients keep working:

```
POST /api/statistics/graphs
StatisticsGraphsRequest {
    scheduler_name: String,
    query: Option<String>,          // search query; None = all cards
    period_days: Option<u32>,       // None = all time; for reviews/hourly/added/buttons
    forecast_days: u32,             // default 30
    date: DateTime<Utc>,            // "now", for tests and the date picker
}
StatisticsGraphsResponse {
    mature_threshold_days: u32,
    today: TodayStats,
    forecast: Vec<ForecastDay>,             // { date, learning, review }
    reviews: Vec<ReviewDay>,                // { date, learning, relearning, young, mature, cram, seconds }
    true_retention: Vec<RetentionRow>,      // { label, young: PassFail, mature: PassFail }
    answer_buttons: AnswerButtons,          // per group: Vec<(RatingId, u32)>
    hourly: [HourStats; 24],                // { reviews, passed, crams, crams_passed }
    card_counts: CardCounts,
    added: Vec<(NaiveDate, u32)>,
    distributions: Distributions,           // pre-bucketed histograms + averages
}
```

- Narrowing to the query reuses `Evaluator::get_card_ids` in the same way `build_review_filter_query`
  does. Write the card ids to a temporary table, or `IN (...)` for small sets, and join on it,
  rather than formatting thousands of ids into each query.
- Bucket on the server so the payload stays small. Histograms should have about 20–40 buckets,
  with the bucket edges included in the response.
- Retrievability needs the scheduler. Add
  `SrsScheduler::retrievability(&self, card: &Card, last_review: DateTime<Utc>, at: DateTime<Utc>) -> Option<f64>`
  with a default of `None`. The FSRS version wraps `schedulers/fsrs/utils.rs::retrievability`,
  which `reposition.rs` already uses. If it returns `None`, the frontend hides that histogram.
- `get_statistics` can later reuse the forecast helper instead of building `due_count_by_date`
  itself. Do that refactor once the forecast numbers have been checked against it.

## Local time

The existing heatmap groups by day with SQLite's `'localtime'`, which is the server's time zone,
and `get_statistics` uses chrono `Local`. `spares deploy` now sets the VPS to the laptop's time
zone (`local_timezone` in `spares_cli/src/deploy/mod.rs`), so for one user this already matches.
It still goes wrong when the user travels or a client is in another zone.

Phases 1–3 group days and hours in the server's local time, like the existing code, so the new
sections agree with the heatmap and the Today tiles. Phase 4 adds `tz: Option<String>` to the
request (an IANA zone, which the frontend fills from
`Intl.DateTimeFormat().resolvedOptions().timeZone`, falling back to `Local`), groups in Rust with
it, and moves the heatmap and `get_statistics` over at the same time.

## Performance

- No migration is needed: `idx_review_log_reviewed_at` covers the period filters and
  `idx_review_log_card_id_reviewed_at` (which includes the rowid `id`) covers the `LAG()` window.
  Check both with `EXPLAIN QUERY PLAN` in the bench.
- Aggregate in SQL with `GROUP BY` wherever the definition allows. The only per-card work in Rust
  should be retrievability, which needs one pass over non-new cards.
- Target: under 300 ms for 50k cards and 500k review logs. Add a criterion bench next to
  `spares_core/benches/update_notes.rs`, with data from `api/tests/fuzz_data.rs`.

## Frontend

- Add `src/components/charts/` with one `BarChart` (stacked bars, optional line) and one
  `Histogram`, both hand-drawn SVG like `ReviewHeatmap.tsx`. Don't add a charting dependency: it
  would grow the bundle that the recent service-worker and compression work keeps small.
  Requirements:
  - Colours are tokens in `index.css` for light and dark mode.
  - Hovering or tapping shows a tooltip.
  - Each chart has a "show as table" toggle for accessibility and for copying numbers.
  - It works at phone width.
- At the top of `StatisticsPage.tsx`, add the query input (with recent searches through
  `useRecentSearches`), period chips (1 month / 3 months / 1 year / all) and a forecast range.
  Keep the date picker; it sets `date`.
- Section order: Today, Forecast, Reviews, True retention, Calendar, Card counts, Answer buttons,
  Hourly, Added, Distributions, Scheduling (advance/postpone).
- Fetch the graphs data separately from the existing `getStatistics` call, so the Today tiles
  don't wait for the heavier request.
- Add the types to `src/types/spares.ts` and the client function to `src/api/client.ts`.

## Offline

The page shows the last statistics it fetched when the server can't be reached. It doesn't
compute statistics on the client: that would mean copying the whole review log into the browser
and duplicating the server's definitions in TypeScript.

- Add a `statistics` object store to `src/offline/db.ts`, which means bumping the database to
  version 2 and creating the store in `onupgradeneeded` when `oldVersion < 2`. Each entry is
  keyed by the server URL, query, period and forecast range, and holds the `StatisticsResponse`,
  the `StatisticsGraphsResponse` and `fetchedAt`.
- Save an entry after every successful fetch. Keep the 10 most recently used and evict the
  oldest, since a graphs response for a full year is a few tens of kB.
- When offline, or when the fetch fails, show the cached entry with a banner like "Offline:
  statistics from 14:05". If some offline actions in the outbox haven't been sent yet, the banner
  also says "3 reviews not counted yet". If `fetchedAt` was before today, the Today section shows
  that date instead of "Today".
- With no cached entry for the current view, list the views that are cached so the user can pick
  one, instead of showing an error.
- The advance/postpone forms in Scheduling need the server, so they are disabled while offline,
  with the same hint `OfflineIndicator` shows elsewhere.
- The CLI talks to the server directly and doesn't change.

## CLI

- `spares card statistics` gets `--query <q>`, `--period <days|all>`, `--forecast-days <n>` and
  `--section <name>...` (default: all).
- Text output: tables for true retention, answer buttons and card counts. Bars drawn with Unicode
  block characters for forecast, reviews, hourly and the histograms, sized to the terminal width
  as `history.rs` does. `--json` prints `StatisticsGraphsResponse`.
- Extend the existing `sections_like_the_frontend` and `crams_are_shown_apart` tests so the CLI
  and web sections stay in step.

## Phases

1. **Core and the highest-value views.** Query filter, the `[statistics] mature_threshold_days`
   setting, forecast, reviews, true retention. Endpoint, frontend sections and CLI sections for
   all three.
2. **Answer buttons, hourly, added, card counts.**
3. **Distributions.** Add `retrievability` and `is_pass` to the scheduler trait, then the
   histograms and total knowledge. Move the calendar onto the page.
4. **Cleanup.** Make `get_statistics` use the shared forecast helper, and add the `tz` request
   field to the new endpoint, the heatmap and `get_statistics` together.
5. **Offline.** Cache the statistics in IndexedDB and show them, with the banner, when the
   server can't be reached.

Each phase can be shipped and committed on its own.

## Tests

- Core (`#[sqlx::test]`): build fixed review logs, including a forget, a cram, an orphaned row, a
  suspended card and a buried card, and assert each section exactly. Cover the young/mature
  boundary at 21 days and at a configured `mature_threshold_days` of 7, a cram between two
  reviews (it must not change the young/mature result or the last review time), a failed cram (it
  lowers the cram % correct in Today and Hourly but not the review one), a query filter that
  matches nothing, and a period that starts mid-history.
- Time zones (phase 4): the same data grouped under two different `tz` values gives the expected
  days and hours.
- Scheduler trait: a scheduler returning `None` from `retrievability` gives an empty
  retrievability histogram, not an error.
- Frontend (`bun test`): bucketing and stacking helpers, and formatting tooltips and tables.
  Offline: a cached entry is shown when the fetch fails, the oldest entry is evicted past 10,
  and upgrading the database from version 1 keeps the queues and outbox.
- CLI: snapshot of the text output at a fixed terminal width.
