# Review, offline refresh, and note-link benchmarks

The [paired samples](review-offline-links.csv) compare Spares before these changes (6c0a6509) with the optimized implementation. All runs used the same arm64 Mac, Rust 1.100.0, and Bun 1.4.5.

## Review

The release benchmark used the statistics fixture with 50,000 cards and 500,000 historical reviews. Each executable received a private copy of the same SQLite file. The clock, filters, rating, and four Rayon threads were fixed. Each of ten cycles fetched a card, rated it, then fetched its successor. Narrow review used the query `c.id<201`. Both executables returned identical card IDs, counts by state, and time estimates at both fetches across all 20 cycles.

| Operation | Median before | Median after | p95 before | p95 after |
|---|---:|---:|---:|---:|
| Normal fetch | 2,650.3 ms | 48.8 ms | 2,718.1 ms | 264.6 ms |
| Normal rating | 9.0 ms | 8.2 ms | 10.0 ms | 10.5 ms |
| Normal fetch after rating | 2,642.5 ms | 49.8 ms | 3,061.9 ms | 51.7 ms |
| Narrow fetch | 2,231.7 ms | 34.8 ms | 2,275.2 ms | 39.2 ms |
| Narrow rating | 9.3 ms | 7.7 ms | 11.3 ms | 8.1 ms |
| Narrow fetch after rating | 2,208.9 ms | 37.0 ms | 2,247.8 ms | 41.4 ms |

The p95 uses nearest rank. With ten samples it is the largest observation, including the first fetch. Instrumentation around the estimate calculation attributed a median 1,826.4 → 29.2 ms to study pace and 786.7 → 11.4 ms to counting and weighting eligible cards in normal review. Narrow review measured 1,820.6 → 28.6 ms and 381.9 → 2.3 ms respectively.

To repeat the native entry-point benchmarks, use the same newly generated fixture path on both revisions with the benchmark harness.

```sh
RAYON_NUM_THREADS=4 SPARES_REVIEW_BENCH_FIXTURE=/tmp/spares-review-benchmark.sqlite \
  cargo bench -p spares_core --bench review_and_links
```

## Offline refresh

In headless Chromium, five independent browser contexts each refreshed 2,000 synthetic files across 128 buckets. A changed-files response, file downloads, and Cache Storage operations were simulated to isolate checkpointing. The old code used real localStorage and the new code used real IndexedDB. Each context performed the initial refresh, a warm unchanged refresh, a one-file edit, and an unchanged refresh in a newly loaded page.

| Refresh | Wall time before → after | Main-thread task time before → after | Metadata written before → after |
|---|---:|---:|---:|
| Initial | 68.3 → 17.9 ms | 75.4 → 12.3 ms | 16,875,583 → 413,231 B |
| Warm, unchanged | 3.7 → 2.0 ms | 4.18 → 2.38 ms | 368,508 → 0 B |
| One file edited | 3.7 → 2.6 ms | 4.13 → 2.11 ms | 368,508 → 303 B |
| New page, unchanged | 3.8 → 4.1 ms | 4.27 → 4.04 ms | 368,508 → 0 B |

Times are medians. Task time is the Chromium DevTools TaskDuration delta bracketing the evaluation, which includes browser instrumentation outside the in-page wall timer. Metadata counts use ASCII fixture strings and the JSON-encoded values and keys passed to IndexedDB. Actual disk usage depends on the browser.

## Note links

The lookup fixture searched 10,000 numbered keywords. Each row times one batch of 100 lookups on an already constructed matcher. Digit sequences are selective in this fixture. Numberless fuzzy searches share a larger candidate set.

| Lookup | 10 distinct strings, before → after | 100 distinct strings, before → after |
|---|---:|---:|
| Exact | 4.859 → 0.244 ms | 3.229 → 0.026 ms |
| Fuzzy | 3.976 → 0.371 ms | 3.498 → 0.210 ms |
| Miss | 3.459 → 0.138 ms | 3.148 → 0.169 ms |

A separate run of the stored-link API included building the matcher, reading and matching 1,000 links, and persisting the results. It took **32.52 → 22.16 ms** with 10 target numbers and **33.41 → 20.93 ms** with 1,000. Each run included exact matches, fuzzy matches, and misses. Both revisions resolved 667 links in each workload.
