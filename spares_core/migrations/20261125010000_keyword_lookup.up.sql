-- Broad keyword searches test membership once per candidate note.
CREATE INDEX idx_note_keyword_note_keyword ON note_keyword(note_id, keyword);
