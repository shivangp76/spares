-- Searches saved by name, for the CLI and the web app to run again. They belong to this replica:
-- they are not synced, and a full sync leaves them as they are.
CREATE TABLE IF NOT EXISTS saved_search (
    name TEXT PRIMARY KEY NOT NULL,
    query TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
