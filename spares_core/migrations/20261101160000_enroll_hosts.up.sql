-- The root's record of every replica of the collection but its own (see
-- `spares_core::sync::devices`): hosts (the laptop is `0`) as well as browsers. Before hosts, only
-- browsers (2 to 4) enrolled.
CREATE TABLE sync_device_new (
    replica INTEGER PRIMARY KEY NOT NULL CHECK (replica BETWEEN 0 AND 7),
    name TEXT NOT NULL,
    enrolled_at INTEGER NOT NULL -- Unix time
);
INSERT INTO sync_device_new (replica, name, enrolled_at)
SELECT replica, name, enrolled_at FROM sync_device;
DROP TABLE sync_device;
ALTER TABLE sync_device_new RENAME TO sync_device;
