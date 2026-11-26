-- Every link as a (note, prerequisite) pair: a prerequisite link (relation 0) points from the note
-- with it to the note it links to, a dependent link (1) the other way, and a related link (2)
-- makes no pair. Only links that matched a note are pairs. The queue's holds, the walks of
-- `prerequisite_of` and `dependent_of`, the note graph's arrows and goal projections all read
-- this, so a link's relation is turned into a direction in one place.
CREATE VIEW IF NOT EXISTS note_prerequisite AS
SELECT parent_note_id AS note_id, linked_note_id AS prerequisite_id, score, id AS link_id
FROM note_link
WHERE relation = 0 AND linked_note_id IS NOT NULL
UNION ALL
SELECT linked_note_id AS note_id, parent_note_id AS prerequisite_id, score, id AS link_id
FROM note_link
WHERE relation = 1 AND linked_note_id IS NOT NULL;

-- A dependent link is followed from the note it links to, so its rows are found by that note.
CREATE INDEX IF NOT EXISTS idx_note_link_linked_note_id ON note_link(linked_note_id);
