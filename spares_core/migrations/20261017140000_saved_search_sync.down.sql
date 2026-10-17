DROP TRIGGER IF EXISTS sync_saved_search_insert;
DROP TRIGGER IF EXISTS sync_saved_search_delete;
DROP TRIGGER IF EXISTS sync_saved_search_before_delete;
DROP TRIGGER IF EXISTS sync_saved_search_update;

-- See the up migration.
PRAGMA legacy_alter_table = ON;

CREATE TABLE sync_base_new (
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card')),
    oid INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (kind, oid)
);
INSERT INTO sync_base_new (kind, oid, snapshot)
SELECT kind, oid, snapshot FROM sync_base WHERE kind != 'saved_search';
DROP TABLE sync_base;
ALTER TABLE sync_base_new RENAME TO sync_base;

CREATE TABLE grave_new (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
);
INSERT INTO grave_new (oid, kind, usn) SELECT oid, kind, usn FROM grave WHERE kind != 'saved_search';
DROP TABLE grave;
ALTER TABLE grave_new RENAME TO grave;

PRAGMA legacy_alter_table = OFF;

CREATE TABLE saved_search_new (
    name TEXT PRIMARY KEY NOT NULL,
    query TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
INSERT INTO saved_search_new (name, query, created_at, updated_at)
SELECT name, query, created_at, updated_at FROM saved_search;
DROP TABLE saved_search;
ALTER TABLE saved_search_new RENAME TO saved_search;
