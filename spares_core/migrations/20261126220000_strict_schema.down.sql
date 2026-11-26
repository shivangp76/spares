-- Reverting the strict schema isn't supported: the code writes every column, so the tables
-- without it still work, but restore a backup instead.
CREATE TEMP TABLE strict_schema_guard (the_strict_schema_can_not_be_reverted INTEGER NOT NULL);
INSERT INTO strict_schema_guard VALUES (NULL);
