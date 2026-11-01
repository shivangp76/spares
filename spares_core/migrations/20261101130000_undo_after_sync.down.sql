-- See the up migration.
DROP TRIGGER IF EXISTS sync_received_parser_insert;
DROP TRIGGER IF EXISTS sync_received_parser_update;
DROP TRIGGER IF EXISTS sync_received_parser_delete;
DROP TRIGGER IF EXISTS sync_received_tag_insert;
DROP TRIGGER IF EXISTS sync_received_tag_update;
DROP TRIGGER IF EXISTS sync_received_tag_delete;
DROP TRIGGER IF EXISTS sync_received_note_insert;
DROP TRIGGER IF EXISTS sync_received_note_update;
DROP TRIGGER IF EXISTS sync_received_note_delete;
DROP TRIGGER IF EXISTS sync_received_card_insert;
DROP TRIGGER IF EXISTS sync_received_card_update;
DROP TRIGGER IF EXISTS sync_received_card_delete;
DROP TRIGGER IF EXISTS sync_received_note_tag_insert;
DROP TRIGGER IF EXISTS sync_received_note_tag_delete;
DROP TRIGGER IF EXISTS sync_received_card_tag_insert;
DROP TRIGGER IF EXISTS sync_received_card_tag_delete;
DROP TRIGGER IF EXISTS sync_received_note_keyword_insert;
DROP TRIGGER IF EXISTS sync_received_note_keyword_delete;
DROP TABLE IF EXISTS sync_received;
DROP TRIGGER IF EXISTS sync_review_log_insert;
DROP TRIGGER IF EXISTS sync_review_log_delete;

PRAGMA legacy_alter_table = ON;

CREATE TABLE grave_new (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
);
INSERT INTO grave_new (oid, kind, usn) SELECT oid, kind, usn FROM grave WHERE kind != 'review_log';
DROP TABLE grave;
ALTER TABLE grave_new RENAME TO grave;

PRAGMA legacy_alter_table = OFF;
