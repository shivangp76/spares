-- The server's record of the browsers that keep a copy of the collection (replicas 2 to 4), so
-- each gets a replica number of its own (`POST /api/sync/enroll`). Unlike `sync_client`, a full
-- upload keeps it: the numbers stay taken until a device is unenrolled.
CREATE TABLE IF NOT EXISTS sync_device (
    replica INTEGER PRIMARY KEY NOT NULL CHECK (replica BETWEEN 2 AND 4),
    name TEXT NOT NULL,
    enrolled_at INTEGER NOT NULL -- Unix time
);
