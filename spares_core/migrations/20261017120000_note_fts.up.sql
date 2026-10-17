-- A full-text index of each note's text and keywords, so searches for words can rank notes by
-- relevance (`bm25`). Searches still match with `REGEXP`, since FTS5 splits and folds text
-- differently, so the index only orders the notes they find.
--
-- It is contentless, so the text is not stored twice, and `contentless_delete` lets the triggers
-- below delete a note's row. It is derived from `note` and `note_keyword` and kept up to date by
-- the triggers, so every write (an edit, an import, undo, sync, a full sync's restore) leaves it
-- right. It is not synced: each replica derives it from its own rows. `rowid` is the note's id.

CREATE VIRTUAL TABLE IF NOT EXISTS note_fts USING fts5(
    data,
    keywords,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO note_fts (rowid, data, keywords)
SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
FROM note n;

CREATE TRIGGER IF NOT EXISTS note_fts_insert AFTER INSERT ON note
BEGIN
    INSERT INTO note_fts (rowid, data, keywords)
    VALUES (NEW.id, NEW.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = NEW.id), ''));
END;

CREATE TRIGGER IF NOT EXISTS note_fts_update AFTER UPDATE OF id, data ON note
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.id;
    INSERT INTO note_fts (rowid, data, keywords)
    VALUES (NEW.id, NEW.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = NEW.id), ''));
END;

CREATE TRIGGER IF NOT EXISTS note_fts_delete AFTER DELETE ON note
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.id;
END;

-- A note's keywords changed: index it again, if it is still there.

CREATE TRIGGER IF NOT EXISTS note_fts_keyword_insert AFTER INSERT ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid = NEW.note_id;
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id = NEW.note_id;
END;

CREATE TRIGGER IF NOT EXISTS note_fts_keyword_delete AFTER DELETE ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.note_id;
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id = OLD.note_id;
END;

CREATE TRIGGER IF NOT EXISTS note_fts_keyword_update AFTER UPDATE OF note_id, keyword ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid IN (OLD.note_id, NEW.note_id);
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id IN (OLD.note_id, NEW.note_id);
END;
