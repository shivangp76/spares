CREATE TABLE sync_device_old (
    replica INTEGER PRIMARY KEY NOT NULL CHECK (replica BETWEEN 2 AND 4),
    name TEXT NOT NULL,
    enrolled_at INTEGER NOT NULL
);
INSERT INTO sync_device_old (replica, name, enrolled_at)
SELECT replica, name, enrolled_at FROM sync_device WHERE replica BETWEEN 2 AND 4;
DROP TABLE sync_device;
ALTER TABLE sync_device_old RENAME TO sync_device;
