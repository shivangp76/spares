-- Reverting past the baseline isn't supported: on a database the old migrations made, it would
-- drop everything. Restore a backup instead.
CREATE TEMP TABLE baseline_guard (the_baseline_migration_can_not_be_reverted INTEGER NOT NULL);
INSERT INTO baseline_guard VALUES (NULL);
