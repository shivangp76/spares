-- Deleted events can't be restored; this only
-- restores the old shape.
DROP VIEW IF EXISTS event_action;
DROP TABLE event;

CREATE TABLE event (
    id INTEGER PRIMARY KEY NOT NULL,
    kind STRING NOT NULL,
    created_at INTEGER
        DEFAULT (strftime('%s', 'now')) NOT NULL,
    version INTEGER NOT NULL,
    group_id INTEGER,
    payload TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_event_group_id
    ON event(group_id);
CREATE INDEX IF NOT EXISTS idx_event_created_at
    ON event(created_at);
CREATE INDEX IF NOT EXISTS idx_event_kind
    ON event(kind);
