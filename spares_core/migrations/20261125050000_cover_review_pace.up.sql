-- The queue reads the previous state and both durations for eligible cards. Keep
-- those values in its existing history index. id preserves the chronological
-- tie-breaker used by latest-review lookups when timestamps are equal.
DROP INDEX idx_review_log_card_id_kind_reviewed_at;
CREATE INDEX idx_review_log_card_id_kind_reviewed_at ON review_log(
    card_id, kind, reviewed_at, id, previous_state, recall_duration, rate_duration
);
