-- Writes `stability_fast` wherever a card is stored without one, so that the code reads every
-- card as it is written now, without a default. Cards scheduled before FSRS-7 have one stability,
-- which is also their fast one (as the fsrs crate takes a memory state without a fast stability).
--
-- - `card` rows: added with the column at 0. Those left at 0 with a stability came from replicas
--   that predated it.
-- - Card snapshots in undo events (`event.payload`) and sync bases (`sync_base.snapshot`): an
--   object with a numeric `stability` and `difficulty` is a card. `UpdateCardPayload` keeps
--   transitions (`{"before", "after"}`) there instead, so it isn't touched.

UPDATE card SET stability_fast = stability WHERE stability_fast = 0 AND stability != 0;

UPDATE sync_base
SET snapshot = json_set(snapshot, '$.stability_fast', json_extract(snapshot, '$.stability'))
WHERE kind = 'card'
    AND json_type(snapshot, '$.stability') IN ('integer', 'real')
    AND json_type(snapshot, '$.stability_fast') IS NULL;

-- An event can keep any number of cards, anywhere in its payload. Their paths are listed once,
-- numbered per event, and each payload then gets one `json_set` per card, in order. Adding a key
-- to a card changes no other card's path.
CREATE TEMP TABLE card_path (
    event_id INTEGER NOT NULL,
    n INTEGER NOT NULL,
    path TEXT NOT NULL,
    stability ANY NOT NULL,
    PRIMARY KEY (event_id, n)
);

INSERT INTO card_path (event_id, n, path, stability)
SELECT e.id, row_number() OVER (PARTITION BY e.id ORDER BY t.id), t.fullkey,
    json_extract(t.value, '$.stability')
FROM event AS e, json_tree(e.payload) AS t
WHERE e.payload LIKE '%"stability"%'
    AND t.type = 'object'
    AND json_type(t.value, '$.stability') IN ('integer', 'real')
    AND json_type(t.value, '$.difficulty') IN ('integer', 'real')
    AND json_type(t.value, '$.stability_fast') IS NULL;

CREATE TEMP TABLE backfilled_event (id INTEGER PRIMARY KEY NOT NULL, payload TEXT NOT NULL);

INSERT INTO backfilled_event (id, payload)
WITH RECURSIVE fixed (id, payload, n) AS (
    SELECT id, payload, 0 FROM event WHERE id IN (SELECT event_id FROM card_path)
    UNION ALL
    SELECT f.id, json_set(f.payload, c.path || '.stability_fast', c.stability), f.n + 1
    FROM fixed AS f
    JOIN card_path AS c ON c.event_id = f.id AND c.n = f.n + 1
)
SELECT f.id, f.payload FROM fixed AS f
WHERE f.n = (SELECT max(c.n) FROM card_path AS c WHERE c.event_id = f.id);

UPDATE event
SET payload = (SELECT payload FROM backfilled_event AS b WHERE b.id = event.id)
WHERE id IN (SELECT id FROM backfilled_event);

DROP TABLE backfilled_event;

DROP TABLE card_path;
