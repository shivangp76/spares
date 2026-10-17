-- Syncs saved searches between replicas, like tags (see `spares_core::sync`). They get an integer
-- id, as sync keys every row by one, and a `usn`, and their name stays unique.
--
-- The searches saved so far were saved on one replica each, and a migration can't tell which
-- replica it runs on, so they can't take ids by the replica's rule (`spares_core::ids`). They get
-- random negative ids instead, which no other replica's searches have and which new ids, counting
-- up from the largest one, never reach. They are new to sync (`usn = -1`), and one saved under the
-- same name on both replicas becomes one search on the first sync.

CREATE TABLE saved_search_new (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    query TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    -- See 20261014130000_sync.
    usn INTEGER NOT NULL DEFAULT -1
);

INSERT INTO saved_search_new (id, name, query, created_at, updated_at)
SELECT -1 - abs(random() % 4503599627370496), name, query, created_at, updated_at
FROM saved_search;

DROP TABLE saved_search;
ALTER TABLE saved_search_new RENAME TO saved_search;
CREATE INDEX IF NOT EXISTS idx_saved_search_usn ON saved_search(usn);

-- `grave.kind` and `sync_base.kind` name the synced tables in a CHECK, which SQLite can't change
-- in place, so both are rebuilt with `saved_search` added, as in 20261015130000_review_log_cram.
-- Triggers on other tables write to them by name, which resolves again once the new tables have
-- the old names. Renaming checks every trigger's tables unless `legacy_alter_table` is on, and
-- they name `grave` and `sync_base`, which don't exist between the drop and the rename.

PRAGMA legacy_alter_table = ON;

CREATE TABLE grave_new (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
);
INSERT INTO grave_new (oid, kind, usn) SELECT oid, kind, usn FROM grave;
DROP TABLE grave;
ALTER TABLE grave_new RENAME TO grave;

CREATE TABLE sync_base_new (
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search')),
    oid INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (kind, oid)
);
INSERT INTO sync_base_new (kind, oid, snapshot) SELECT kind, oid, snapshot FROM sync_base;
DROP TABLE sync_base;
ALTER TABLE sync_base_new RENAME TO sync_base;

PRAGMA legacy_alter_table = OFF;

-- Change tracking, as for tags in 20261014130000_sync and 20261015120000_tag_reschedule.

CREATE TRIGGER IF NOT EXISTS sync_saved_search_update AFTER UPDATE ON saved_search
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.query IS NOT NEW.query)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('saved_search', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'query', OLD.query,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
    UPDATE saved_search SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS sync_saved_search_before_delete BEFORE DELETE ON saved_search
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('saved_search', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'query', OLD.query,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
END;

CREATE TRIGGER IF NOT EXISTS sync_saved_search_delete AFTER DELETE ON saved_search
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'saved_search', -1);
END;

CREATE TRIGGER IF NOT EXISTS sync_saved_search_insert AFTER INSERT ON saved_search
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'saved_search' AND oid = NEW.id;
END;
