DROP TRIGGER IF EXISTS review_log_previous_interval_update;
DROP TRIGGER IF EXISTS review_log_previous_interval_delete;
DROP TRIGGER IF EXISTS review_log_previous_interval_insert;
DROP INDEX IF EXISTS idx_review_log_card_id_kind_reviewed_at;
ALTER TABLE review_log DROP COLUMN previous_interval;
