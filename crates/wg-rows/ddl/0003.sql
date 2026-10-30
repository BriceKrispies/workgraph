-- Revision 0003 (stacks): the statements Alembic's `0003` upgrade runs, in order: the `stacks`, `stack_items` and `stack_repositories` tables and their indexes.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE stacks (
	id VARCHAR NOT NULL, 
	workspace_id VARCHAR NOT NULL, 
	parent_task_id VARCHAR NOT NULL, 
	status VARCHAR(16) NOT NULL, 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_stacks PRIMARY KEY (id), 
	CONSTRAINT fk_stacks_parent_task_id_tasks FOREIGN KEY(parent_task_id) REFERENCES tasks (id) ON DELETE RESTRICT, 
	CONSTRAINT fk_stacks_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT
)
----
CREATE UNIQUE INDEX ix_stacks_active_parent ON stacks (parent_task_id) WHERE status NOT IN ('landed', 'abandoned')
----
CREATE INDEX ix_stacks_parent_task_id ON stacks (parent_task_id)
----
CREATE INDEX ix_stacks_workspace_id ON stacks (workspace_id)
----
CREATE TABLE stack_items (
	id VARCHAR NOT NULL, 
	stack_id VARCHAR NOT NULL, 
	task_id VARCHAR NOT NULL, 
	state VARCHAR(16) NOT NULL, 
	merge_order INTEGER, 
	detail TEXT, 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_stack_items PRIMARY KEY (id), 
	CONSTRAINT fk_stack_items_stack_id_stacks FOREIGN KEY(stack_id) REFERENCES stacks (id) ON DELETE CASCADE, 
	CONSTRAINT fk_stack_items_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE RESTRICT, 
	CONSTRAINT uq_stack_items_stack_id UNIQUE (stack_id, task_id)
)
----
CREATE INDEX ix_stack_items_stack_id ON stack_items (stack_id)
----
CREATE INDEX ix_stack_items_task_id ON stack_items (task_id)
----
CREATE TABLE stack_repositories (
	stack_id VARCHAR NOT NULL, 
	repository_id VARCHAR NOT NULL, 
	base_sha VARCHAR(64) NOT NULL, 
	integration_branch VARCHAR(300) NOT NULL, 
	created_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_stack_repositories PRIMARY KEY (stack_id, repository_id), 
	CONSTRAINT fk_stack_repositories_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE RESTRICT, 
	CONSTRAINT fk_stack_repositories_stack_id_stacks FOREIGN KEY(stack_id) REFERENCES stacks (id) ON DELETE CASCADE
)
----
CREATE INDEX ix_stack_repositories_repository_id ON stack_repositories (repository_id)
