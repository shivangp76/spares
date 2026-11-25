# Note browsing, Typst editing, statistics, and review performance

Measurements compare baseline `672d0e19` with the changes through `a3b05b4b` on one arm64 Mac. Native results use release builds and four Rayon threads. Each process received a private copy of the same 50,000-note, 50,000-card, 500,000-review fixture. The harness finished migrations, checkpointed SQLite, then reopened a copy before the first fetch. The operating system file cache was left enabled. [Individual samples](notes-editor-statistics.csv)

| Workload | Before | After | Change |
|---|---:|---:|---:|
| Notes page, 100 notes with generated links | 4.63 ms | 0.85 ms | 82% less time |
| Default note graph, capped at 2,000 nodes | 35.47 ms | 8.91 ms | 75% less time |
| Typst typing, p95 keydown to next animation frame | 88.5 ms | 78.6 ms | 11% less time |
| Forecast change, hosted and offline | 3 requests | 1 request | 67% fewer |
| First normal review fetch | 241.26 ms | 107.52 ms | 55% less time |
| Subsequent normal review fetch | 19.30 ms | 15.85 ms | 18% less time |
| Normal rating alone | 7.69 ms | 10.27 ms | 2.58 ms longer |
| Normal rating plus fetching its successor | 29.01 ms | 27.45 ms | 5% less time |
| Rating plus successor, filter restricted to 200 card IDs | 17.25 ms | 15.92 ms | 8% less time |

## Native measurements

Three processes ran per revision in the order baseline, optimized, optimized, baseline, baseline, optimized. Times are pooled medians. Browsing uses calls 2–12 from each run. The first normal review fetch includes an empty study-day cache. Each process then completed ten normal and ten filtered fetch–rate–fetch cycles. Combined times sum the rating and its successor fetch within each cycle.

The 100-note page went from 100 link queries to one. The graph seed query returned 2,001 IDs, down from 50,000. The extra ID establishes whether the graph was truncated. Serialized note pages and complete graph responses had identical digests across revisions and runs. All review cycles agreed on both card IDs, counts by state, and estimated seconds.

A separate SQLx statement trace timed the first normal review fetch. The query that checks eligible cards’ review history fell from 176.68 to 40.32 ms. Building the study-day rows took 17.28 and 18.30 ms respectively. Selecting the next card took 4.30 and 3.96 ms. The wider history index contains both answer durations and the card’s previous state, which lets SQLite calculate per-card averages from the index.

Building the index during upgrade took 471 ms in the traced run. The checkpointed database grew from 69,894,144 to 73,756,672 bytes, about 3.7 MiB. Hosts and devices need the same migration version to sync.

## Browser measurements

Two fresh Chromium profiles ran per revision, alternating in baseline, optimized, optimized, baseline order. Each ran the production frontend with its matching server. The Typst note contained 1,500 lines and the collection contained 10,000 keywords. Playwright typed into ordinary prose, verified that all characters appeared, and timed keydown to the next animation-frame callback with Chromium CPU throttled 4×. The p95 uses nearest rank over 120 measured keystrokes per revision after five warm-ups per run. This measures a frame opportunity. Main-thread task time over the same input intervals, including instrumentation, fell from 94.85 to 85.00 ms at p95.

Each browser changed the forecast to 90, 7, and 365 days and waited for the displayed forecast to confirm each change. The three changes issued nine statistics requests on the baseline and three on the optimized build. Results agreed for HTTP requests and worker calls to an enrolled copy with the network disabled.

## Reproduction

Use the same harness on both revisions, a shared fixture generated on the baseline, and distinct Cargo target directories for the two builds. Set `RUST_LOG=sqlx::query=debug` on a separate native run to collect statement timings. For the browser run, build the server and frontend from the same revision.

```sh
RAYON_NUM_THREADS=4 SPARES_REVIEW_BENCH_FIXTURE=/tmp/spares-review-fixture.sqlite \
  cargo bench -p spares_core --bench review_and_links
cargo build -p spares_server
cd spares_frontend/frontend
bun run build
SPARES_BROWSING_BENCH=1 bun e2e/local_copy.ts chromium
```
