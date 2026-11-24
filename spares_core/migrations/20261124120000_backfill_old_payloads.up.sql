-- Writes into old rows and undo events the fields that the code used to default when they were
-- missing, so it can read every row as it is written now:
--
-- - `origin` in a reschedule's `RescheduleData` (`review_log.custom_data` of a `Reschedule` row,
--   kind 17, and the same in the review logs an undo event keeps). Moves written before origins
--   were recorded were all postponements or advances, which the direction of the move tells apart.
-- - `reschedule` in a tag an undo event keeps. Tags written before it existed all rescheduled.

UPDATE review_log
SET custom_data = json_set(
    custom_data,
    '$.origin',
    CASE
        WHEN json_extract(custom_data, '$.due') >= json_extract(custom_data, '$.previous_due')
            THEN 'postpone'
        ELSE 'advance'
    END
)
WHERE kind = 17
    AND json_type(custom_data, '$.previous_due') = 'integer'
    AND json_type(custom_data, '$.origin') IS NULL;

-- An event can keep any number of either, anywhere in its payload, so each payload is fixed one
-- object at a time until none is left. `json_tree`'s `path` is the object holding the key.
CREATE TEMP TABLE backfilled_event (id INTEGER PRIMARY KEY NOT NULL, payload TEXT NOT NULL);

INSERT INTO backfilled_event (id, payload)
WITH RECURSIVE fixed (id, payload, step) AS (
    SELECT id, payload, 0
    FROM event
    WHERE payload LIKE '%"previous_due"%'
    UNION ALL
    SELECT
        fixed.id,
        json_set(
            fixed.payload,
            t.path || '.origin',
            CASE
                WHEN json_extract(fixed.payload, t.path || '.due')
                    >= json_extract(fixed.payload, t.path || '.previous_due')
                    THEN 'postpone'
                ELSE 'advance'
            END
        ),
        fixed.step + 1
    FROM fixed
    JOIN json_tree(fixed.payload) AS t
        ON t.id = (
            SELECT u.id FROM json_tree(fixed.payload) AS u
            WHERE u.key = 'previous_due'
                AND u.type = 'integer'
                AND json_type(fixed.payload, u.path || '.due') = 'integer'
                AND json_type(fixed.payload, u.path || '.origin') IS NULL
            ORDER BY u.id
            LIMIT 1
        )
)
SELECT id, payload FROM (
    SELECT id, payload, step, max(step) OVER (PARTITION BY id) AS last FROM fixed
)
WHERE step = last AND last > 0;

UPDATE event
SET payload = (SELECT payload FROM backfilled_event AS b WHERE b.id = event.id)
WHERE id IN (SELECT id FROM backfilled_event);

DELETE FROM backfilled_event;

-- A tag is an object with a boolean `auto_delete` and a text `name`. An `UpdateTag` event's
-- changes are objects of `{"b": before, "a": after}` instead, so they aren't touched.
INSERT INTO backfilled_event (id, payload)
WITH RECURSIVE fixed (id, payload, step) AS (
    SELECT id, payload, 0
    FROM event
    WHERE payload LIKE '%"auto_delete"%'
    UNION ALL
    SELECT fixed.id, json_set(fixed.payload, t.path || '.reschedule', json('true')), fixed.step + 1
    FROM fixed
    JOIN json_tree(fixed.payload) AS t
        ON t.id = (
            SELECT u.id FROM json_tree(fixed.payload) AS u
            WHERE u.key = 'auto_delete'
                AND u.type IN ('true', 'false')
                AND json_type(fixed.payload, u.path || '.name') = 'text'
                AND json_type(fixed.payload, u.path || '.reschedule') IS NULL
            ORDER BY u.id
            LIMIT 1
        )
)
SELECT id, payload FROM (
    SELECT id, payload, step, max(step) OVER (PARTITION BY id) AS last FROM fixed
)
WHERE step = last AND last > 0;

UPDATE event
SET payload = (SELECT payload FROM backfilled_event AS b WHERE b.id = event.id)
WHERE id IN (SELECT id FROM backfilled_event);

DROP TABLE backfilled_event;
