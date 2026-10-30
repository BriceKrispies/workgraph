-- Revision 0004 (stack landing): the statements Alembic's `0004` upgrade runs, in order: the nullable `stacks.land_request` column.
-- Statements are separated by a line holding `----`.
----
ALTER TABLE stacks ADD COLUMN land_request TEXT
