-- A note or card has a tag once: the code adds tags with `INSERT OR IGNORE`. Collections created
-- before `note_tag` and `card_tag` had their `UNIQUE` constraints lack them, so this drops any
-- repeated link, keeping the first, and adds the unique indexes the baseline creates. Deleting a
-- repeat only marks its note or card as changed for sync, whose tags stay the same.
DELETE FROM note_tag
WHERE id NOT IN (SELECT MIN(id) FROM note_tag GROUP BY note_id, tag_id);

DELETE FROM card_tag
WHERE id NOT IN (SELECT MIN(id) FROM card_tag GROUP BY card_id, tag_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_note_tag_note_id_tag_id ON note_tag(note_id, tag_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_card_tag_card_id_tag_id ON card_tag(card_id, tag_id);
