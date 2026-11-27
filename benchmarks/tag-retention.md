# Retention per tag

The Tags page's retention bars (`spares_core::api::statistics::tag_retention`) are worked out on each visit, with no cache, as they take well under the 50 ms aimed for.

Measured on 2026-11-27 on one arm64 Mac, with a release build, on a copy of a real collection: 11,425 cards, 16,682 reviews and 289 tags, 155 of which (parents included) have reviewed cards. Over 21 calls, the median was 20.1 ms (19.7 to 24.2 ms), the two SQL queries and the pass over the cards included.

An earlier sketch of it in SQL alone, which joined each tag to its descendants with `LIKE` and used a simplified forgetting curve, took 540 ms on the same collection. Most of that was expanding descendants per tag. This version works out each card's retrievability once, with the scheduler's curve, and adds it to each of its tags and their ancestors, which come from splitting the names at `:`.

## Reproduction

A directory with a copy of the collection (`sqlite3 -readonly <db> ".backup <dir>/collection.sqlite"`) and its `config/config.toml`, without `[sync]`:

```sh
SPARES_TAG_RETENTION_BENCH=<dir> \
  cargo test --release -p spares_core --lib -- --ignored tag_retention_bench --nocapture
```
