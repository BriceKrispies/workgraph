-- Revision 0002 (automation run inputs): the statements Alembic's `0002` upgrade runs, in order: the `automation_runs.inputs` column, added NOT NULL with the default `{}`.
-- Statements are separated by a line holding `----`.
----
ALTER TABLE automation_runs ADD COLUMN inputs TEXT DEFAULT '{}' NOT NULL
