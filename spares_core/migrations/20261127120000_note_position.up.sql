-- The order notes are learned in, worked out from their links and when they were created (see
-- `spares_core::api::note_position`). Like links, positions are not synced: each machine works
-- them out from its own.
CREATE TABLE "note_position" (
    note_id INTEGER PRIMARY KEY NOT NULL,
    position INTEGER NOT NULL,
    -- Whether the note is in a loop of links, within which only links to older notes count
    in_link_cycle INTEGER NOT NULL CHECK (in_link_cycle IN (0, 1)),
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE
) STRICT;
CREATE INDEX idx_note_position_position ON note_position(position);

-- `token` is replaced whenever positions may change. `built_token` and `built_with` are the token
-- and the settings positions were last worked out with; a new database has none.
CREATE TABLE "note_position_version" (
    token BLOB NOT NULL,
    built_token BLOB,
    built_with TEXT NOT NULL
) STRICT;
INSERT INTO note_position_version (token, built_token, built_with) VALUES (randomblob(16), NULL, '');

CREATE TRIGGER note_position_note_insert AFTER INSERT ON note
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;

CREATE TRIGGER note_position_note_delete AFTER DELETE ON note
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;

CREATE TRIGGER note_position_note_created_at AFTER UPDATE OF created_at ON note
WHEN OLD.created_at IS NOT NEW.created_at
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;

CREATE TRIGGER note_position_link_insert AFTER INSERT ON note_link
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;

CREATE TRIGGER note_position_link_delete AFTER DELETE ON note_link
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;

CREATE TRIGGER note_position_link_update AFTER UPDATE ON note_link
BEGIN
    UPDATE note_position_version SET token = randomblob(16);
END;
