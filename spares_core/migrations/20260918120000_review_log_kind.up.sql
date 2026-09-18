-- Adds `review_log.kind` so replay-affecting actions other than reviews (currently only Forget)
-- live in the same ordered log that `SrsScheduler::compute_memory_state` folds over. Without it,
-- forgetting a card leaves no trace in the review log and replay reconstructs the stability and
-- difficulty the card would have had if the forget had never happened.
--
-- This is a NEW migration rather than an edit to 20240326145400_main_data.up.sql because every
-- statement there is `CREATE TABLE IF NOT EXISTS`: editing that CREATE TABLE block would never
-- apply to an existing database, and would break sqlx's checksum validation (see
-- `spares_server/src/main.rs`).
--
-- The table is rebuilt rather than ALTERed because SQLite cannot relax a NOT NULL constraint in
-- place, and `rating` / `scheduled_time` / `recall_duration` / `rate_duration` are meaningless for
-- non-Review rows. See <https://www.sqlite.org/lang_altertable.html#otherALTER>. NULL is used
-- rather than a 0 sentinel so that SQL three-valued logic excludes these rows from every
-- rating-predicated query automatically; 0 is a structurally legal `RatingId` and would silently
-- match predicates like `c.rated<2`.
--
-- `PRAGMA foreign_keys` is deliberately not toggled: it is a no-op inside a transaction, and no
-- table references `review_log`, so dropping and renaming it cannot orphan anything.

CREATE TABLE review_log_new (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    -- Enum. 0 = Review, 1 = Forget. See `ReviewLogKind` in `model.rs`.
    -- 2..=15 are reserved for future replay-affecting kinds.
    kind INTEGER NOT NULL DEFAULT 0 CHECK (kind BETWEEN 0 AND 15),
    rating INTEGER,          -- NULL unless `kind` is Review
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER,  -- NULL unless `kind` is Review
    recall_duration INTEGER, -- NULL unless `kind` is Review
    rate_duration INTEGER,   -- NULL unless `kind` is Review
    previous_state INTEGER NOT NULL,
    -- The filtered tag this row was produced under, if any.
    tag_id INTEGER,
    custom_data TEXT NOT NULL, -- JSON string
    -- A review always has a rating; other kinds never do.
    CHECK ((kind = 0) = (rating IS NOT NULL)),
    -- Do _NOT_ delete review logs when cards are deleted. We want to know how many cards were
    -- reviewed in the past for historical reasons. Instead, set the `card_id` column to null, to
    -- signify the row is an orphan.
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL,
    -- Likewise, deleting a filtered tag must not delete the reviews done under it.
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE SET NULL
);

-- `id` is copied explicitly: stored `RateCard` undo payloads in `event.payload` reference it.
INSERT INTO review_log_new
    (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
     recall_duration, rate_duration, previous_state, tag_id, custom_data)
SELECT
    id, card_id, reviewed_at, 0, rating, scheduler_name, scheduled_time,
    recall_duration, rate_duration, previous_state, NULL, custom_data
FROM review_log;

-- Abort the whole migration if a single row was lost. A CHECK violation raises an error, which
-- rolls the transaction back. `review_log` is the one table in this schema that cannot be rebuilt
-- from the note files, so the copy is verified rather than trusted.
CREATE TEMP TABLE review_log_migration_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO review_log_migration_guard (ok)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM review_log_new) = (SELECT COUNT(*) FROM review_log)
     AND (SELECT COALESCE(MAX(id), 0) FROM review_log_new)
       = (SELECT COALESCE(MAX(id), 0) FROM review_log)
    THEN 1 ELSE 0 END;
DROP TABLE review_log_migration_guard;

DROP TABLE review_log;
ALTER TABLE review_log_new RENAME TO review_log;

-- The index the schedulers' per-card `WHERE card_id = ? ORDER BY reviewed_at ASC` fetches have
-- always wanted; there was none.
CREATE INDEX IF NOT EXISTS idx_review_log_card_id_reviewed_at ON review_log(card_id, reviewed_at);

-- Date-range scans: statistics and the new-cards-per-day limit.
CREATE INDEX IF NOT EXISTS idx_review_log_reviewed_at ON review_log(reviewed_at);

-- Filtered-tag lookups.
CREATE INDEX IF NOT EXISTS idx_review_log_tag_id ON review_log(tag_id);
