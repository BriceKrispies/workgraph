-- Revision 0006 (experiments): the statements Alembic's `0006` upgrade runs, in order: the `experiments` and `experiment_trials` tables and their indexes.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE experiments (
	id VARCHAR NOT NULL, 
	workspace_id VARCHAR NOT NULL, 
	slug VARCHAR(100) NOT NULL, 
	title VARCHAR(500) NOT NULL, 
	question TEXT, 
	status VARCHAR(16) NOT NULL, 
	protocol TEXT NOT NULL, 
	frozen_at VARCHAR(32), 
	created_at VARCHAR(32) NOT NULL, 
	updated_at VARCHAR(32) NOT NULL, 
	CONSTRAINT pk_experiments PRIMARY KEY (id), 
	CONSTRAINT fk_experiments_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT, 
	CONSTRAINT uq_experiments_workspace_id UNIQUE (workspace_id, slug)
)
----
CREATE INDEX ix_experiments_workspace_id ON experiments (workspace_id)
----
CREATE TABLE experiment_trials (
	id VARCHAR NOT NULL, 
	experiment_id VARCHAR NOT NULL, 
	task_id VARCHAR NOT NULL, 
	attempt_id VARCHAR(36), 
	subagent_run_id VARCHAR(36), 
	arm VARCHAR(64) NOT NULL, 
	requested_model VARCHAR(200) NOT NULL, 
	actual_model VARCHAR(200), 
	phase VARCHAR(16) NOT NULL, 
	seq INTEGER NOT NULL, 
	band INTEGER, 
	features TEXT NOT NULL, 
	base_ref VARCHAR(300), 
	worktree_path VARCHAR(1000), 
	status VARCHAR(16) NOT NULL, 
	outcome VARCHAR(8), 
	outcome_detail TEXT NOT NULL, 
	failure_cause VARCHAR(32), 
	failure_cause_source VARCHAR(16), 
	notes TEXT, 
	created_at VARCHAR(32) NOT NULL, 
	started_at VARCHAR(32), 
	graded_at VARCHAR(32), 
	CONSTRAINT pk_experiment_trials PRIMARY KEY (id), 
	CONSTRAINT fk_experiment_trials_experiment_id_experiments FOREIGN KEY(experiment_id) REFERENCES experiments (id) ON DELETE CASCADE, 
	CONSTRAINT fk_experiment_trials_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE RESTRICT, 
	CONSTRAINT uq_experiment_trials_experiment_id UNIQUE (experiment_id, seq)
)
----
CREATE INDEX ix_experiment_trials_experiment_id ON experiment_trials (experiment_id)
----
CREATE INDEX ix_experiment_trials_task_id ON experiment_trials (task_id)
