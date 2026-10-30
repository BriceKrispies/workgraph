-- Revision 0009 (move tasks.external_refs into task_links): the statements Alembic's `0009` upgrade runs, in order: drops `tasks.external_refs`. The data move that fills `task_links` from the old column is a `DataStep` in `migrations.rs`, run around this statement the way Alembic's Python `upgrade()` ran it (rows are read before the drop and inserted after it).
-- Statements are separated by a line holding `----`.
----
ALTER TABLE tasks DROP COLUMN external_refs
