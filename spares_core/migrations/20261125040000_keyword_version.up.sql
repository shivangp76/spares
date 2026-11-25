CREATE TABLE keyword_version (token BLOB NOT NULL);
INSERT INTO keyword_version VALUES (randomblob(16));
CREATE TRIGGER keyword_version_insert AFTER INSERT ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;
CREATE TRIGGER keyword_version_delete AFTER DELETE ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;
CREATE TRIGGER keyword_version_update AFTER UPDATE OF note_id, keyword ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;
