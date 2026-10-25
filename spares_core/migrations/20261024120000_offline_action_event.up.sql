-- The event an applied offline action was recorded as, to undo it with. NULL for an action that
-- was rejected or a duplicate, for one recorded no event, and for those received before this
-- column. Cleared when the event is (e.g. by a full sync).
ALTER TABLE offline_action ADD COLUMN event_id INTEGER REFERENCES event(id) ON DELETE SET NULL;
