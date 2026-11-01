CREATE TABLE IF NOT EXISTS offline_action (
    client_id TEXT PRIMARY KEY NOT NULL,
    -- Unix time
    performed_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    event_id INTEGER REFERENCES event(id) ON DELETE SET NULL
);
