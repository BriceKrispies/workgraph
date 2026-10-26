-- Revision 0011 (task decisions): the statements Alembic's `0011` upgrade runs, in order: the `task_decisions` table and its indexes.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE task_decisions (
	id VARCHAR NOT NULL, 
	workspace_id VARCHAR NOT NULL, 
	task_id VARCHAR NOT NULL, 
	question TEXT NOT NULL, 
	options TEXT NOT NULL, 
	status VARCHAR(16) NOT NULL, 
	chosen_option_id VARCHAR(64), 
	other_text TEXT, 
	note TEXT, 
	decided_by VARCHAR(200), 
	decided_at VARCHAR(32), 
	created_by VARCHAR(200) NOT NULL, 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	version INTEGER NOT NULL, 
	CONSTRAINT pk_task_decisions PRIMARY KEY (id), 
	CONSTRAINT fk_task_decisions_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE, 
	CONSTRAINT fk_task_decisions_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT
)
----
CREATE INDEX ix_task_decisions_status ON task_decisions (status)
----
CREATE INDEX ix_task_decisions_task_id ON task_decisions (task_id)
----
CREATE INDEX ix_task_decisions_workspace_id ON task_decisions (workspace_id)
