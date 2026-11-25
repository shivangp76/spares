-- Derived locally for the configured day boundary and answer-time cap. Card presets
-- are joined at read time so moving a note between presets is immediately visible.
CREATE TABLE study_day_cache (id INTEGER PRIMARY KEY CHECK (id = 1), since INTEGER NOT NULL, rules TEXT NOT NULL);
CREATE TABLE study_day (
    card_id INTEGER NOT NULL, day INTEGER NOT NULL, state INTEGER NOT NULL,
    reviews INTEGER NOT NULL, seconds INTEGER NOT NULL,
    PRIMARY KEY(card_id, day)
) WITHOUT ROWID;
CREATE INDEX study_day_recent ON study_day(state, day DESC, card_id);
CREATE TABLE study_day_dirty (card_id INTEGER PRIMARY KEY);
CREATE TRIGGER study_day_insert AFTER INSERT ON review_log
WHEN NEW.card_id IS NOT NULL AND NEW.kind = 0 AND NEW.reviewed_at >= (SELECT since FROM study_day_cache)
BEGIN
    INSERT INTO study_day_dirty VALUES (NEW.card_id) ON CONFLICT DO NOTHING;
END;
CREATE TRIGGER study_day_delete AFTER DELETE ON review_log
WHEN OLD.card_id IS NOT NULL AND OLD.kind = 0 AND OLD.reviewed_at >= (SELECT since FROM study_day_cache)
BEGIN
    INSERT INTO study_day_dirty VALUES (OLD.card_id) ON CONFLICT DO NOTHING;
END;
CREATE TRIGGER study_day_update AFTER UPDATE OF card_id, reviewed_at, kind, previous_state, recall_duration, rate_duration, id ON review_log
BEGIN
    INSERT INTO study_day_dirty SELECT OLD.card_id WHERE OLD.card_id IS NOT NULL AND OLD.kind = 0
        AND OLD.reviewed_at >= (SELECT since FROM study_day_cache) ON CONFLICT DO NOTHING;
    INSERT INTO study_day_dirty SELECT NEW.card_id WHERE NEW.card_id IS NOT NULL AND NEW.kind = 0
        AND NEW.reviewed_at >= (SELECT since FROM study_day_cache) ON CONFLICT DO NOTHING;
END;
