-- Revision 0008 (trackers): the statements Alembic's `0008` upgrade runs, in order: the `external_items` and `task_links` tables and their indexes.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE external_items (
	id VARCHAR NOT NULL, 
	workspace_id VARCHAR NOT NULL, 
	task_id VARCHAR NOT NULL, 
	connection VARCHAR(100) NOT NULL, 
	external_id VARCHAR(200) NOT NULL, 
	external_key VARCHAR(64) NOT NULL, 
	url VARCHAR(1000) NOT NULL, 
	tracker_status VARCHAR(200) NOT NULL, 
	tracker_status_category VARCHAR(32) NOT NULL, 
	tracker_resolution VARCHAR(200), 
	remote_updated_at VARCHAR(32), 
	snapshot TEXT NOT NULL, 
	snapshot_hash VARCHAR(64) NOT NULL, 
	synced_at VARCHAR(32), 
	missing_since VARCHAR(32), 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_external_items PRIMARY KEY (id), 
	CONSTRAINT fk_external_items_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT, 
	CONSTRAINT fk_external_items_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE, 
	CONSTRAINT uq_external_items_connection UNIQUE (connection, external_id), 
	CONSTRAINT uq_external_items_task_id UNIQUE (task_id)
)
----
CREATE INDEX ix_external_items_workspace_id ON external_items (workspace_id)
----
CREATE INDEX ix_external_items_external_key ON external_items (external_key)
----
CREATE TABLE task_links (
	id VARCHAR NOT NULL, 
	task_id VARCHAR NOT NULL, 
	kind VARCHAR(32) NOT NULL, 
	provider VARCHAR(100) NOT NULL, 
	url VARCHAR(1000) NOT NULL, 
	normalized_url VARCHAR(1000) NOT NULL, 
	repo_full_name VARCHAR(300) NOT NULL, 
	number INTEGER, 
	repository_id VARCHAR, 
	sources TEXT NOT NULL, 
	title TEXT NOT NULL, 
	created_by VARCHAR(200) NOT NULL, 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_task_links PRIMARY KEY (id), 
	CONSTRAINT fk_task_links_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE, 
	CONSTRAINT fk_task_links_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE SET NULL, 
	CONSTRAINT uq_task_links_task_id UNIQUE (task_id, kind, normalized_url)
)
----
CREATE INDEX ix_task_links_task_id ON task_links (task_id)
----
CREATE INDEX ix_task_links_normalized_url ON task_links (normalized_url)
----
CREATE INDEX ix_task_links_repository_id ON task_links (repository_id)
