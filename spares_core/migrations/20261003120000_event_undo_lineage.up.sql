-- Rebuilds event so redo is possible. Deletes all
-- existing events (undo history only; other tables
-- are untouched). New migration rather than an edit
-- to main_data: see 20260918120000_review_log_kind.
--
-- reverts_event_id is the event this row reverses:
-- an undo points at its action, a redo at its undo.
-- UNIQUE: an event can be reverted at most once.
-- Do/undo/redo is the parity of chain depth, derived
-- by the event_action view. version is dropped
-- since nothing read it.

DROP TABLE event;

CREATE TABLE event (
    id INTEGER PRIMARY KEY NOT NULL,
    kind STRING NOT NULL,
    -- Unix time
    created_at INTEGER
        DEFAULT (strftime('%s', 'now')) NOT NULL,
    group_id INTEGER,
    reverts_event_id INTEGER UNIQUE,
    payload TEXT NOT NULL, -- JSON
    FOREIGN KEY (reverts_event_id)
        REFERENCES event(id)
);

CREATE INDEX IF NOT EXISTS idx_event_group_id
    ON event(group_id);
CREATE INDEX IF NOT EXISTS idx_event_created_at
    ON event(created_at);
CREATE INDEX IF NOT EXISTS idx_event_kind
    ON event(kind);

-- 0 = Do, 1 = Undo, 2 = Redo (EventAction).
-- Odd depth = undo, even nonzero depth = redo.
CREATE VIEW event_action AS
WITH RECURSIVE chain(id, depth) AS (
    SELECT id, 0 FROM event
    WHERE reverts_event_id IS NULL
    UNION ALL
    SELECT e.id, c.depth + 1
    FROM event e
    JOIN chain c ON e.reverts_event_id = c.id
)
SELECT id,
    CASE WHEN depth = 0 THEN 0
         WHEN depth % 2 = 1 THEN 1
         ELSE 2 END AS action
FROM chain;
