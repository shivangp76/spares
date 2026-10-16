-- Allows `review_log.kind = 16` (`ReviewLogKind::Cram` in `model.rs`): a rating in a filtered tag
-- that does not reschedule, which is recorded for statistics but skipped by every replay of the
-- card's history. Before this, such ratings were not logged at all, so cramming left the
-- statistics and the heatmap blank.
--
-- Two CHECKs stood in the way: `kind BETWEEN 0 AND 15`, and `(kind = 0) = (rating IS NOT NULL)`,
-- which only allowed a rating on a Review. SQLite cannot change a CHECK in place, so the table is
-- rebuilt, as in 20260918120000_review_log_kind (which explains why this is a new migration rather
-- than an edit to an old one, and why `PRAGMA foreign_keys` is not toggled). No table references
-- `review_log`, and no trigger is defined on it.

CREATE TABLE review_log_new (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    -- Enum. 0 = Review, 1 = Forget, 16 = Cram. See `ReviewLogKind` in `model.rs`.
    -- 0..=15 are replayed into a card's memory state, 16..=31 only recorded; the rest of each
    -- range is reserved for future kinds.
    kind INTEGER NOT NULL DEFAULT 0 CHECK (kind BETWEEN 0 AND 31),
    rating INTEGER,          -- NULL unless `kind` is Review or Cram
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER,  -- NULL unless `kind` is Review
    recall_duration INTEGER, -- NULL unless `kind` is Review or Cram
    rate_duration INTEGER,   -- NULL unless `kind` is Review or Cram
    previous_state INTEGER NOT NULL,
    -- The filtered tag this row was produced under, if any.
    tag_id INTEGER,
    custom_data TEXT NOT NULL, -- JSON string
    -- See 20261014130000_sync.
    usn INTEGER NOT NULL DEFAULT -1,
    -- A review or a cram always has a rating; other kinds never do.
    CHECK ((kind IN (0, 16)) = (rating IS NOT NULL)),
    -- Do _NOT_ delete review logs when cards are deleted. We want to know how many cards were
    -- reviewed in the past for historical reasons. Instead, set the `card_id` column to null, to
    -- signify the row is an orphan.
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL,
    -- Likewise, deleting a filtered tag must not delete the reviews done under it.
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE SET NULL
);

-- `id` is copied explicitly: stored `RateCard` undo payloads in `event.payload` reference it.
-- `usn` too, so that the rebuild does not mark every row as changed for sync.
INSERT INTO review_log_new
    (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
     recall_duration, rate_duration, previous_state, tag_id, custom_data, usn)
SELECT
    id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
    recall_duration, rate_duration, previous_state, tag_id, custom_data, usn
FROM review_log;

-- Abort the whole migration if a single row was lost (see 20260918120000_review_log_kind).
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

-- Dropping the table dropped its indexes, so they are recreated as they were.
CREATE INDEX IF NOT EXISTS idx_review_log_card_id_reviewed_at ON review_log(card_id, reviewed_at);
CREATE INDEX IF NOT EXISTS idx_review_log_reviewed_at ON review_log(reviewed_at);
CREATE INDEX IF NOT EXISTS idx_review_log_tag_id ON review_log(tag_id);
CREATE INDEX IF NOT EXISTS idx_review_log_usn ON review_log(usn);
