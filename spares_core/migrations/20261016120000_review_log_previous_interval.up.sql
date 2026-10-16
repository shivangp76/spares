-- Each review's previous interval: the `scheduled_time` of the card's previous Review-kind row,
-- ordered by `(reviewed_at, id)`. Statistics tell young and mature reviews apart by it.
--
-- It is derived from the other rows, and kept up to date here by triggers rather than by the code
-- that writes reviews, so every write (a review, undo, sync, an offline rating placed before a
-- later review, a card's deletion orphaning its reviews) leaves it right. Computing it in each
-- query instead, with a `LAG()` window, takes most of a second over 500k reviews.
--
-- `NULL` on every row that is not a Review (`kind = 0`), on orphaned rows (`card_id IS NULL`),
-- and on a card's first review. It is not synced: each replica derives it from its own rows.

ALTER TABLE review_log ADD COLUMN previous_interval INTEGER;

UPDATE review_log SET previous_interval = previous.scheduled_time
FROM (
    SELECT id, LAG(scheduled_time) OVER (PARTITION BY card_id ORDER BY reviewed_at, id)
        AS scheduled_time
    FROM review_log WHERE kind = 0 AND card_id IS NOT NULL
) AS previous
WHERE review_log.id = previous.id AND previous.scheduled_time IS NOT NULL;

-- A card's reviews of one kind in order, so its previous or next Review, or its last one, is a
-- seek that reads only the index. Through `idx_review_log_card_id_reviewed_at`, each crammed or
-- forgotten row on the way is a lookup of the table to skip it, which makes finding the last
-- review of 50k cards take a third of a second.
CREATE INDEX IF NOT EXISTS idx_review_log_card_id_kind_reviewed_at
    ON review_log(card_id, kind, reviewed_at);

-- Each trigger recomputes the rows whose previous Review may have changed: the row itself, and the
-- Review that follows it, at its old place and at its new one.

CREATE TRIGGER IF NOT EXISTS review_log_previous_interval_insert AFTER INSERT ON review_log
WHEN NEW.kind = 0 AND NEW.card_id IS NOT NULL
BEGIN
    UPDATE review_log SET previous_interval = (
        SELECT p.scheduled_time FROM review_log p
        WHERE p.card_id = review_log.card_id AND p.kind = 0
          AND p.reviewed_at <= review_log.reviewed_at
          AND (p.reviewed_at < review_log.reviewed_at OR p.id < review_log.id)
        ORDER BY p.reviewed_at DESC, p.id DESC LIMIT 1)
    WHERE id IN (NEW.id, (
        SELECT n.id FROM review_log n
        WHERE n.card_id = NEW.card_id AND n.kind = 0
          AND n.reviewed_at >= NEW.reviewed_at
          AND (n.reviewed_at > NEW.reviewed_at OR n.id > NEW.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1));
END;

CREATE TRIGGER IF NOT EXISTS review_log_previous_interval_delete AFTER DELETE ON review_log
WHEN OLD.kind = 0 AND OLD.card_id IS NOT NULL
BEGIN
    UPDATE review_log SET previous_interval = OLD.previous_interval
    WHERE id = (
        SELECT n.id FROM review_log n
        WHERE n.card_id = OLD.card_id AND n.kind = 0
          AND n.reviewed_at >= OLD.reviewed_at
          AND (n.reviewed_at > OLD.reviewed_at OR n.id > OLD.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1);
END;

-- Not on `previous_interval`, so the updates above do not set it off again.
CREATE TRIGGER IF NOT EXISTS review_log_previous_interval_update
AFTER UPDATE OF card_id, reviewed_at, kind, scheduled_time ON review_log
BEGIN
    UPDATE review_log SET previous_interval = CASE
        WHEN review_log.kind = 0 AND review_log.card_id IS NOT NULL THEN (
            SELECT p.scheduled_time FROM review_log p
            WHERE p.card_id = review_log.card_id AND p.kind = 0
              AND p.reviewed_at <= review_log.reviewed_at
              AND (p.reviewed_at < review_log.reviewed_at OR p.id < review_log.id)
            ORDER BY p.reviewed_at DESC, p.id DESC LIMIT 1)
        END
    WHERE id IN (NEW.id, (
        SELECT n.id FROM review_log n
        WHERE n.card_id = OLD.card_id AND n.kind = 0 AND n.id != NEW.id
          AND n.reviewed_at >= OLD.reviewed_at
          AND (n.reviewed_at > OLD.reviewed_at OR n.id > OLD.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1), (
        SELECT n.id FROM review_log n
        WHERE n.card_id = NEW.card_id AND n.kind = 0 AND n.id != NEW.id
          AND n.reviewed_at >= NEW.reviewed_at
          AND (n.reviewed_at > NEW.reviewed_at OR n.id > NEW.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1));
END;
