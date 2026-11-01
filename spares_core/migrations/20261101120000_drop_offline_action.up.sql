-- Web clients without a copy of the collection no longer review offline: a device that keeps a
-- copy schedules its own reviews and syncs them. The ids of the actions they submitted later are
-- no longer needed.
DROP TABLE IF EXISTS offline_action;
