-- Reverses the `review_log.kind` rebuild.
--
-- Unlike 20240326145400_main_data.down.sql this does NOT drop the data: it rebuilds the pre-`kind`
-- schema and copies every row back. `review_log` is the one table in this schema that cannot be
-- reconstructed from the note files.
--
-- Two things cannot be represented in the old schema and would otherwise be silently dropped:
-- Forget rows (no `kind` column there, and `rating` is NOT NULL), and `tag_id` (no such column
-- existed before this migration). This refuses to run while either is present rather than losing
-- data quietly. Export or delete the offending rows first if you really mean to revert.

CREATE TEMP TABLE review_log_revert_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO review_log_revert_guard (ok)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM review_log WHERE kind != 0) = 0
     AND (SELECT COUNT(*) FROM review_log WHERE tag_id IS NOT NULL) = 0
    THEN 1 ELSE 0 END;
DROP TABLE review_log_revert_guard;

CREATE TABLE review_log_old (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    rating INTEGER NOT NULL,
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER NOT NULL,
    recall_duration INTEGER NOT NULL,
    rate_duration INTEGER NOT NULL,
    previous_state INTEGER NOT NULL,
    custom_data TEXT NOT NULL, -- JSON string
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL
);

INSERT INTO review_log_old
    (id, card_id, reviewed_at, rating, scheduler_name, scheduled_time,
     recall_duration, rate_duration, previous_state, custom_data)
SELECT
    id, card_id, reviewed_at, rating, scheduler_name, scheduled_time,
    recall_duration, rate_duration, previous_state, custom_data
FROM review_log;

DROP TABLE review_log;
ALTER TABLE review_log_old RENAME TO review_log;
