-- The collection's settings: the part of `config.toml` that isn't about one machine, as TOML
-- text (see `spares_core::settings`). Its one row syncs like the other tables (see
-- `spares_core::sync`), with its last synced text in `base` rather than in `sync_base`, whose
-- kinds it isn't one of. A database has no row until its collection's `config.toml` is first
-- moved in.
CREATE TABLE "collection_settings" (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    text TEXT NOT NULL,
    -- The text as last synced, while the row has changed since; `NULL` otherwise
    base TEXT,
    updated_at INTEGER NOT NULL,
    usn INTEGER NOT NULL
) STRICT;

CREATE TRIGGER sync_collection_settings_update AFTER UPDATE ON collection_settings
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND OLD.text IS NOT NEW.text
BEGIN
    UPDATE collection_settings SET usn = -1, base = COALESCE(OLD.base, OLD.text) WHERE id = NEW.id;
END;
