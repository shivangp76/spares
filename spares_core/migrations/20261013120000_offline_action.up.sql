-- Study actions that web clients performed offline and submitted later (see
-- `api::offline_review::sync_offline_actions`). A client retries a batch until it gets a response,
-- so the same action can arrive more than once. `client_id` is the id the client gave it, and an
-- action whose id is already here is not applied again.
CREATE TABLE IF NOT EXISTS offline_action (
    client_id TEXT PRIMARY KEY NOT NULL,
    -- Unix time
    performed_at INTEGER NOT NULL,
    received_at INTEGER NOT NULL
);
