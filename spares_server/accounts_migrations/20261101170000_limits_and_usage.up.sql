-- What a user may use, where it differs from host.toml's [limits] (NULL: the host's), and what
-- they use (see `spares_server::host::limits`).
CREATE TABLE user_limits (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    sync_body_mb INTEGER,
    media_body_mb INTEGER,
    storage_mb INTEGER,
    requests_per_minute INTEGER,
    search_seconds INTEGER,
    renders_at_once INTEGER
) STRICT;

CREATE TABLE usage (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    -- The size of the user's directory when last measured, and when (Unix time)
    storage_bytes INTEGER NOT NULL DEFAULT 0,
    storage_measured_at INTEGER,
    -- Requests since the account was created
    requests INTEGER NOT NULL DEFAULT 0,
    last_request_at INTEGER
) STRICT;
