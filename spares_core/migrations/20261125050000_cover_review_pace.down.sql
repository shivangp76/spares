DROP INDEX idx_review_log_card_id_kind_reviewed_at;
CREATE INDEX idx_review_log_card_id_kind_reviewed_at ON review_log(card_id, kind, reviewed_at);
