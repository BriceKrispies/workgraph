-- Revision 0010 (task files): the statements Alembic's `0010` upgrade runs, in order: the `task_files` table and its index.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE task_files (
	task_id VARCHAR NOT NULL, 
	position INTEGER NOT NULL, 
	path VARCHAR(1000) NOT NULL, 
	anchor VARCHAR(300), 
	note TEXT NOT NULL, 
	CONSTRAINT pk_task_files PRIMARY KEY (task_id, position), 
	CONSTRAINT fk_task_files_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE
)
----
CREATE INDEX ix_task_files_path ON task_files (path)
