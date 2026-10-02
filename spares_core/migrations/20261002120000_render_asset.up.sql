-- Files that browser-rendered parsers (Typst) need to compile a card but that are not generated
-- from the database: preambles imported by the card template, figures, rendered image
-- occlusions, ... They are discovered with `typst compile --deps` while rendering locally.
--
-- The contents are not stored here. They live content-addressed at `<data dir>/render_assets/<hash>`
-- so large images do not bloat the database. `path` is the absolute path the source refers to,
-- which the browser compiler maps the contents back onto.
CREATE TABLE IF NOT EXISTS render_asset (
    path TEXT PRIMARY KEY NOT NULL,
    hash TEXT NOT NULL, -- blake3 hex of the contents
    mtime INTEGER NOT NULL, -- Nanoseconds since the Unix epoch. Used to skip rehashing unchanged files.
    size INTEGER NOT NULL
);

-- Typst packages (e.g. `preview/cetz/0.5.2`) seen while rendering. The browser prefetches these
-- before compiling since the compiler resolves packages synchronously.
CREATE TABLE IF NOT EXISTS render_package (
    spec TEXT PRIMARY KEY NOT NULL
);
