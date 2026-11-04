-- What rendering this machine's files of each note recorded: the render key that tells whether a
-- rendered file is up to date, and the files and Typst packages it read (see
-- `spares_core::api::render_state`). Each machine renders its own files, so this isn't synced, and
-- a full sync doesn't copy it.
CREATE TABLE IF NOT EXISTS render_output (
    note_id INTEGER NOT NULL,
    -- '' for the note's own file, '<card order>-front' or '<card order>-back' for a card's
    output TEXT NOT NULL,
    key TEXT NOT NULL,
    -- `RenderDependencies` as JSON: {"files": [...], "packages": [...]}
    dependencies TEXT NOT NULL,
    PRIMARY KEY (note_id, output),
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE
);
