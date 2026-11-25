# Review, browsing, and offline manifest performance

The native comparisons use release builds of `d579c982` and `27c3fe0e` on the same arm64 Mac, with four Rayon threads. Each process received its own copy of the same SQLite fixture with 50,000 cards and 500,000 reviews. Two runs of each binary alternated in the order baseline, optimized, optimized, baseline. The measurements below are medians. [Individual samples](review-browsing.csv)

| Workload | Before | After | Change |
|---|---:|---:|---:|
| Normal review fetch | 49.65 ms | 21.34 ms | 57.0% less time |
| Fetch the next card after rating | 51.05 ms | 23.31 ms | 54.3% less time |
| Fetch the next card after rating, filtered to 200 card IDs | 38.72 ms | 9.80 ms | 74.7% less time |
| Unchanged offline manifest, 2,000 files | 7.90 ms | 0.024 ms | 99.7% less time |
| Interactive keyword search with a number | 10.66 ms | 0.076 ms | 99.3% less time |
| Interactive keyword search without a number | 11.31 ms | 0.762 ms | 93.3% less time |
| Page of 800 sibling cards | 13.62 ms | 13.54 ms | 0.6% less time |
| Browser rating click to next rendered front, 40 ms added per review API request | 111.55 ms | 61.05 ms | 45.3% less time |

## Review

Each run performed ten fetch–rate–fetch cycles per filter with a fixed clock and rating. Card IDs, counts by state, and estimated seconds agreed between revisions at both fetches in every cycle. Normal rating alone took 9.46 → 8.97 ms. Narrow rating alone took 8.43 → 9.26 ms. The first normal fetch, which includes cache initialization, took 256–264 ms before and 237–254 ms after.

The browser comparison isolates the extra request using the same optimized host and production frontend for both flows. Headless Chromium rated 24 cards in a disposable collection with 40 Markdown cards. The benchmark alternated whether the rating requested its successor and added 40 ms before forwarding each rating or selection request. It timed from the rating click until the next distinct front's HTML appeared in its iframe on an animation frame. It verified exactly one rating per transition and that separate selection requests went from one to zero. Network latency, document size, and rendering engine affect the visible gain.

### Rating to visible card

The current browser benchmark also delays the rendered front's file request by 40 ms and checks the loading page on each animation frame until the new front is visible. On the 40-card Markdown fixture, sending the front HTML with the rating response reduced the median from 111.45 ms to 61.05 ms over eight interleaved samples per flow. The median across six more inline transitions with no added latency was 26.80 ms. The host includes HTML fronts up to 64 KiB. Larger fronts use the file request.

## Browsing and offline files

Each native run made twelve calls per workload. The table uses calls 2–12 to show repeated use.

- The rendered-file test creates 2,000 real HTML files, calls the server's manifest function, and submits the returned bucket digests on subsequent calls. It excludes HTTP and browser storage. The first listing took 9.00–9.45 ms before and 12.14–13.16 ms after, including watcher setup. A collection periodically reconciles its cached manifest against the filesystem.
- The keyword fixture contains 10,000 entries. Both searches are fuzzy. The numbered search returns one row and the numberless search returns 5,000. The timed API call includes fetching or checking the index, scoring candidates, and sorting the results. Building the index on the first numbered query took 10.70–13.13 ms including the search, compared with 11.49–12.48 ms previously.
- The card page contains eight siblings from each of 100 long UTF-8 notes. All 800 previews matched the first 200 characters. The note body column passed from SQLite to Rust fell from **9,299,200 to 212,800 bytes** on that page, a **97.7% reduction**. The end-to-end page time showed little change.

## Reproduction

Use the same harness on both native revisions and generate the reusable fixture on the baseline first. The harness migrates a private copy before measurements. Run the browser benchmark against a built server and frontend.

```sh
RAYON_NUM_THREADS=4 SPARES_REVIEW_BENCH_FIXTURE=/tmp/spares-review-benchmark.sqlite \
  cargo bench -p spares_core --bench review_and_links
cargo build -p spares_server
cd spares_frontend/frontend
bun run build
bun e2e/review_transition.ts
```
