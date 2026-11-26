DROP VIEW sync_card_snapshot;
CREATE VIEW sync_card_snapshot AS
SELECT c.id AS id, json_object(
    'id', c.id,
    'note_id', c.note_id,
    'order', c."order",
    'back_type', c.back_type,
    'created_at', c.created_at,
    'updated_at', c.updated_at,
    'due', c.due,
    'stability', c.stability,
    'difficulty', c.difficulty,
    'desired_retention', c.desired_retention,
    'special_state', c.special_state,
    'state', c.state,
    'custom_data', json(c.custom_data),
    'tags', (SELECT json_group_array(tag_id) FROM
        (SELECT tag_id FROM card_tag WHERE card_id = c.id ORDER BY tag_id))
) AS snapshot
FROM card c;

DROP TRIGGER sync_card_update;
CREATE TRIGGER sync_card_update AFTER UPDATE ON card
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.note_id IS NOT NEW.note_id OR OLD."order" IS NOT NEW."order"
        OR OLD.back_type IS NOT NEW.back_type OR OLD.due IS NOT NEW.due
        OR OLD.stability IS NOT NEW.stability OR OLD.difficulty IS NOT NEW.difficulty
        OR OLD.desired_retention IS NOT NEW.desired_retention
        OR OLD.special_state IS NOT NEW.special_state OR OLD.state IS NOT NEW.state
        OR OLD.custom_data IS NOT NEW.custom_data OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('card', OLD.id, json_object(
        'id', OLD.id,
        'note_id', OLD.note_id,
        'order', OLD."order",
        'back_type', OLD.back_type,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at,
        'due', OLD.due,
        'stability', OLD.stability,
        'difficulty', OLD.difficulty,
        'desired_retention', OLD.desired_retention,
        'special_state', OLD.special_state,
        'state', OLD.state,
        'custom_data', json(OLD.custom_data),
        'tags', (SELECT json_group_array(tag_id) FROM
            (SELECT tag_id FROM card_tag WHERE card_id = OLD.id ORDER BY tag_id))
    ));
    UPDATE card SET usn = -1 WHERE id = NEW.id;
END;

DROP TRIGGER sync_received_card_update;
CREATE TRIGGER sync_received_card_update AFTER UPDATE ON card
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

ALTER TABLE card DROP COLUMN stability_fast;
