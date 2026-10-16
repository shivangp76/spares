-- Reverses the cram rebuild, copying every row back into the previous schema.
--
-- Cram rows cannot be represented there (their `kind` is out of range, and only a Review may have
-- a rating), so this refuses to run while any exist rather than dropping them quietly. Delete them
-- first (`DELETE FROM review_log WHERE kind = 16`) if you really mean to revert.

CREATE TEMP TABLE review_log_revert_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO review_log_revert_guard (ok)
SELECT CASE WHEN (SELECT COUNT(*) FROM review_log WHERE kind = 16) = 0 THEN 1 ELSE 0 END;
DROP TABLE review_log_revert_guard;

CREATE TABLE review_log_old (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    kind INTEGER NOT NULL DEFAULT 0 CHECK (kind BETWEEN 0 AND 15),
    rating INTEGER,
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER,
    recall_duration INTEGER,
    rate_duration INTEGER,
    previous_state INTEGER NOT NULL,
    tag_id INTEGER,
    custom_data TEXT NOT NULL,
    usn INTEGER NOT NULL DEFAULT -1,
    CHECK ((kind = 0) = (rating IS NOT NULL)),
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL,
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE SET NULL
);

INSERT INTO review_log_old
    (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
     recall_duration, rate_duration, previous_state, tag_id, custom_data, usn)
SELECT
    id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time,
    recall_duration, rate_duration, previous_state, tag_id, custom_data, usn
FROM review_log;

DROP TABLE review_log;
ALTER TABLE review_log_old RENAME TO review_log;

CREATE INDEX IF NOT EXISTS idx_review_log_card_id_reviewed_at ON review_log(card_id, reviewed_at);
CREATE INDEX IF NOT EXISTS idx_review_log_reviewed_at ON review_log(reviewed_at);
CREATE INDEX IF NOT EXISTS idx_review_log_tag_id ON review_log(tag_id);
CREATE INDEX IF NOT EXISTS idx_review_log_usn ON review_log(usn);
