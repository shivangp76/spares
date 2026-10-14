-- Change tracking for two-way sync (section 5 of docs/sync-plan.md, see `spares_core::sync`).
--
-- `usn` ("update sequence number") on every synced table:
--   -1  changed on this replica since it was last synced. New rows get it from the default, and
--       the triggers below set it on updates, so no write path can be missed.
--   >=0 unchanged since the sync that stamped it with the server's counter.
--
-- The triggers stay out of the way of sync itself: while sync applies the other replica's
-- changes it puts a row in `sync_applying`, inside its own transaction, so no other connection
-- ever sees it. (A trigger cannot read a TEMP table, so this is a regular table.)

ALTER TABLE parser ADD COLUMN usn INTEGER NOT NULL DEFAULT -1;
ALTER TABLE tag ADD COLUMN usn INTEGER NOT NULL DEFAULT -1;
ALTER TABLE note ADD COLUMN usn INTEGER NOT NULL DEFAULT -1;
ALTER TABLE card ADD COLUMN usn INTEGER NOT NULL DEFAULT -1;
ALTER TABLE review_log ADD COLUMN usn INTEGER NOT NULL DEFAULT -1;

CREATE INDEX IF NOT EXISTS idx_parser_usn ON parser(usn);
CREATE INDEX IF NOT EXISTS idx_tag_usn ON tag(usn);
CREATE INDEX IF NOT EXISTS idx_note_usn ON note(usn);
CREATE INDEX IF NOT EXISTS idx_card_usn ON card(usn);
CREATE INDEX IF NOT EXISTS idx_review_log_usn ON review_log(usn);
CREATE INDEX IF NOT EXISTS idx_card_note_id ON card(note_id);
CREATE INDEX IF NOT EXISTS idx_card_tag_card_id ON card_tag(card_id);

-- Rows deleted on this replica. Without them, "deleted over there" would look the same as "not
-- synced yet", and sync would bring the row back. `kind` is the table name.
CREATE TABLE IF NOT EXISTS grave (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
);

-- The last synced version of a row changed or deleted since, which the merge compares both sides
-- against. Taken by the triggers on the first change after a sync. `snapshot` is the row as JSON in the
-- shape of `spares_core::sync::records` (for a note: with its tag ids and its keywords that are
-- not embedded in the text; for a card: with its tag ids).
CREATE TABLE IF NOT EXISTS sync_base (
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card')),
    oid INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (kind, oid)
);

-- Notes whose text both replicas changed in the same lines. The local text stays in the note;
-- the other replica's is kept here until `spares sync conflicts` resolves it. JSON snapshots.
CREATE TABLE IF NOT EXISTS sync_conflict (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    base TEXT,
    local TEXT NOT NULL,
    remote TEXT NOT NULL,
    created_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL -- Unix time
);
CREATE INDEX IF NOT EXISTS idx_sync_conflict_note_id ON sync_conflict(note_id);

-- Holds one row while sync applies changes, which turns the triggers off. Never committed.
CREATE TABLE IF NOT EXISTS sync_applying (
    applying INTEGER NOT NULL
);

-- The server's record of the last sync of each client replica. A client that reports an older
-- one (restored from a backup) has to do a full sync.
CREATE TABLE IF NOT EXISTS sync_client (
    replica INTEGER PRIMARY KEY NOT NULL,
    last_usn INTEGER NOT NULL,
    last_sync_at INTEGER NOT NULL -- Unix time
);

-- Snapshots of the current rows, in the shape `sync_base` stores. The triggers use them for
-- parents whose child rows change; the sync code uses them to read records.
CREATE VIEW IF NOT EXISTS sync_note_snapshot AS
SELECT n.id AS id, json_object(
    'id', n.id,
    'data', n.data,
    'created_at', n.created_at,
    'updated_at', n.updated_at,
    'custom_data', json(n.custom_data),
    'parser_id', n.parser_id,
    'tags', (SELECT json_group_array(tag_id) FROM
        (SELECT tag_id FROM note_tag WHERE note_id = n.id ORDER BY tag_id)),
    'keywords', (SELECT json_group_array(keyword) FROM
        (SELECT keyword FROM note_keyword WHERE note_id = n.id AND embedded = 0 ORDER BY keyword))
) AS snapshot
FROM note n;

CREATE VIEW IF NOT EXISTS sync_card_snapshot AS
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

-- ---- Updates of synced rows ----
-- Fire only when a synced column changed (not `updated_at` alone) and the row is not dirty yet.

CREATE TRIGGER IF NOT EXISTS sync_parser_update AFTER UPDATE ON parser
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND OLD.name IS NOT NEW.name
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('parser', OLD.id, json_object('id', OLD.id, 'name', OLD.name));
    UPDATE parser SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_tag_update AFTER UPDATE ON tag
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.description IS NOT NEW.description
        OR OLD.query IS NOT NEW.query OR OLD.auto_delete IS NOT NEW.auto_delete)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('tag', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'description', OLD.description,
        'query', OLD.query,
        'auto_delete', OLD.auto_delete,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
    UPDATE tag SET usn = -1 WHERE id = NEW.id;
END;

-- The child rows are still the old ones here: a change to them first marks the note dirty
-- (below), and then this trigger does not fire.
CREATE TRIGGER IF NOT EXISTS sync_note_update AFTER UPDATE ON note
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.data IS NOT NEW.data OR OLD.custom_data IS NOT NEW.custom_data
        OR OLD.parser_id IS NOT NEW.parser_id OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('note', OLD.id, json_object(
        'id', OLD.id,
        'data', OLD.data,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at,
        'custom_data', json(OLD.custom_data),
        'parser_id', OLD.parser_id,
        'tags', (SELECT json_group_array(tag_id) FROM
            (SELECT tag_id FROM note_tag WHERE note_id = OLD.id ORDER BY tag_id)),
        'keywords', (SELECT json_group_array(keyword) FROM
            (SELECT keyword FROM note_keyword
             WHERE note_id = OLD.id AND embedded = 0 ORDER BY keyword))
    ));
    UPDATE note SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_card_update AFTER UPDATE ON card
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

-- ---- Child rows: a note's tags and its keywords that are not embedded, a card's tags ----
-- BEFORE triggers, so the snapshot still has the old children. When the parent is being
-- deleted, or the tag is, the cascade runs after that row is gone, so the joins below skip it:
-- deleting a tag does not mark every note that had it as changed.

CREATE TRIGGER IF NOT EXISTS sync_note_tag_insert BEFORE INSERT ON note_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = NEW.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = NEW.note_id AND usn != -1;
END;

CREATE TRIGGER IF NOT EXISTS sync_note_tag_delete BEFORE DELETE ON note_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    AND EXISTS (SELECT 1 FROM tag WHERE id = OLD.tag_id)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = OLD.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = OLD.note_id AND usn != -1;
END;

CREATE TRIGGER IF NOT EXISTS sync_note_keyword_insert BEFORE INSERT ON note_keyword
WHEN NEW.embedded = 0 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = NEW.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = NEW.note_id AND usn != -1;
END;

CREATE TRIGGER IF NOT EXISTS sync_note_keyword_delete BEFORE DELETE ON note_keyword
WHEN OLD.embedded = 0 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = OLD.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = OLD.note_id AND usn != -1;
END;

CREATE TRIGGER IF NOT EXISTS sync_card_tag_insert BEFORE INSERT ON card_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', s.id, s.snapshot FROM sync_card_snapshot s JOIN card c ON c.id = s.id
    WHERE s.id = NEW.card_id AND c.usn != -1;
    UPDATE card SET usn = -1 WHERE id = NEW.card_id AND usn != -1;
END;

CREATE TRIGGER IF NOT EXISTS sync_card_tag_delete BEFORE DELETE ON card_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    AND EXISTS (SELECT 1 FROM tag WHERE id = OLD.tag_id)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', s.id, s.snapshot FROM sync_card_snapshot s JOIN card c ON c.id = s.id
    WHERE s.id = OLD.card_id AND c.usn != -1;
    UPDATE card SET usn = -1 WHERE id = OLD.card_id AND usn != -1;
END;

-- ---- Deletes and re-inserts ----
-- A delete keeps the row's synced version as its base, taken before the delete so a note or a
-- card still has its tags, and leaves a grave. Inserting a row under an id that has a grave (undo
-- of a delete, redo of a create) removes the grave again; its base stays, and is the right one.

CREATE TRIGGER IF NOT EXISTS sync_parser_before_delete BEFORE DELETE ON parser
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('parser', OLD.id, json_object('id', OLD.id, 'name', OLD.name));
END;

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
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
END;

CREATE TRIGGER IF NOT EXISTS sync_note_before_delete BEFORE DELETE ON note
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', id, snapshot FROM sync_note_snapshot WHERE id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_card_before_delete BEFORE DELETE ON card
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', id, snapshot FROM sync_card_snapshot WHERE id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_parser_delete AFTER DELETE ON parser
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'parser', -1);
END;

CREATE TRIGGER IF NOT EXISTS sync_tag_delete AFTER DELETE ON tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'tag', -1);
END;

CREATE TRIGGER IF NOT EXISTS sync_note_delete AFTER DELETE ON note
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'note', -1);
END;

CREATE TRIGGER IF NOT EXISTS sync_card_delete AFTER DELETE ON card
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'card', -1);
END;

CREATE TRIGGER IF NOT EXISTS sync_parser_insert AFTER INSERT ON parser
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'parser' AND oid = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_tag_insert AFTER INSERT ON tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'tag' AND oid = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_note_insert AFTER INSERT ON note
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'note' AND oid = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_card_insert AFTER INSERT ON card
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'card' AND oid = NEW.id;
END;
