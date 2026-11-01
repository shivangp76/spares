-- Undo past a sync (see `spares_core::api::undo`).
--
-- 1. Review logs get graves, so a review that an undo deletes is deleted on the other replicas
--    too. Until now only an undo before the next sync could delete one, and review logs were only
--    ever added between replicas.
-- 2. `sync_received` records each row that sync wrote, with the newest event at the time. An
--    event can be undone as long as sync wrote none of the rows it changed after it: nothing from
--    the other side would be overwritten. A card's change counts for its note too, and a change
--    to a note's or card's tags or a note's keywords that aren't embedded for the note or card.
--    Writes that only stamp a usn don't count.

-- What stopped undo at the last sync before
DELETE FROM app_state WHERE key = 'last_sync_event_id';

-- `grave.kind` names the synced tables in a CHECK, which SQLite can't change in place (see
-- 20261017140000_saved_search_sync).
PRAGMA legacy_alter_table = ON;

CREATE TABLE grave_new (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search', 'review_log')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
);
INSERT INTO grave_new (oid, kind, usn) SELECT oid, kind, usn FROM grave;
DROP TABLE grave;
ALTER TABLE grave_new RENAME TO grave;

PRAGMA legacy_alter_table = OFF;

CREATE TRIGGER IF NOT EXISTS sync_review_log_delete AFTER DELETE ON review_log
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'review_log', -1);
END;

-- A redo puts an undone review back under its id
CREATE TRIGGER IF NOT EXISTS sync_review_log_insert AFTER INSERT ON review_log
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'review_log' AND oid = NEW.id;
END;

-- `kind` is `note`, `card`, `tag` or `parser`. Rows from before the oldest event can't stop an
-- undo, so a sync drops them.
CREATE TABLE IF NOT EXISTS sync_received (
    kind TEXT NOT NULL,
    oid INTEGER NOT NULL,
    event_id INTEGER NOT NULL,
    PRIMARY KEY (kind, oid)
);

CREATE TRIGGER IF NOT EXISTS sync_received_parser_insert AFTER INSERT ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_parser_update AFTER UPDATE ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND OLD.name IS NOT NEW.name
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_parser_delete AFTER DELETE ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_tag_insert AFTER INSERT ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_tag_update AFTER UPDATE ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.description IS NOT NEW.description
        OR OLD.query IS NOT NEW.query OR OLD.auto_delete IS NOT NEW.auto_delete
        OR OLD.reschedule IS NOT NEW.reschedule)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_tag_delete AFTER DELETE ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_insert AFTER INSERT ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_update AFTER UPDATE ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.data IS NOT NEW.data OR OLD.custom_data IS NOT NEW.custom_data
        OR OLD.parser_id IS NOT NEW.parser_id OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_delete AFTER DELETE ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_card_insert AFTER INSERT ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_card_update AFTER UPDATE ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.note_id IS NOT NEW.note_id OR OLD."order" IS NOT NEW."order"
        OR OLD.back_type IS NOT NEW.back_type OR OLD.due IS NOT NEW.due
        OR OLD.stability IS NOT NEW.stability OR OLD.difficulty IS NOT NEW.difficulty
        OR OLD.desired_retention IS NOT NEW.desired_retention
        OR OLD.special_state IS NOT NEW.special_state OR OLD.state IS NOT NEW.state
        OR OLD.custom_data IS NOT NEW.custom_data OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_card_delete AFTER DELETE ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_tag_insert AFTER INSERT ON note_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_tag_delete AFTER DELETE ON note_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_card_tag_insert AFTER INSERT ON card_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.card_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    -- None when the card is being deleted, which counts for its note itself
    INSERT INTO sync_received (kind, oid, event_id)
    SELECT 'note', note_id, (SELECT COALESCE(MAX(id), 0) FROM event) FROM card WHERE id = NEW.card_id
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_card_tag_delete AFTER DELETE ON card_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', OLD.card_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    -- None when the card is being deleted, which counts for its note itself
    INSERT INTO sync_received (kind, oid, event_id)
    SELECT 'note', note_id, (SELECT COALESCE(MAX(id), 0) FROM event) FROM card WHERE id = OLD.card_id
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_keyword_insert AFTER INSERT ON note_keyword
WHEN EXISTS (SELECT 1 FROM sync_applying) AND NEW.embedded = 0
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER IF NOT EXISTS sync_received_note_keyword_delete AFTER DELETE ON note_keyword
WHEN EXISTS (SELECT 1 FROM sync_applying) AND OLD.embedded = 0
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;
