-- Whether rating a card while studying a filtered tag reschedules it (see `Tag::reschedule`).
ALTER TABLE tag ADD COLUMN reschedule INTEGER NOT NULL DEFAULT 1;

-- The sync triggers snapshot every synced column, so they are recreated with the new one.
DROP TRIGGER IF EXISTS sync_tag_update;
CREATE TRIGGER IF NOT EXISTS sync_tag_update AFTER UPDATE ON tag
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.description IS NOT NEW.description
        OR OLD.query IS NOT NEW.query OR OLD.auto_delete IS NOT NEW.auto_delete
        OR OLD.reschedule IS NOT NEW.reschedule)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('tag', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'description', OLD.description,
        'query', OLD.query,
        'auto_delete', OLD.auto_delete,
        'reschedule', OLD.reschedule,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
    UPDATE tag SET usn = -1 WHERE id = NEW.id;
END;

DROP TRIGGER IF EXISTS sync_tag_before_delete;
CREATE TRIGGER IF NOT EXISTS sync_tag_before_delete BEFORE DELETE ON tag
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('tag', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'description', OLD.description,
        'query', OLD.query,
        'auto_delete', OLD.auto_delete,
        'reschedule', OLD.reschedule,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
END;
