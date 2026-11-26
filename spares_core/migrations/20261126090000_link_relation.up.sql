-- Which way a link points (`LinkRelation`): 0 when the note with the link builds on the note it
-- links to, 1 when the linked note builds on it, 2 when neither does. Links written before there
-- was a way to say so are all the first kind.
ALTER TABLE note_link ADD COLUMN relation INTEGER NOT NULL DEFAULT 0;
