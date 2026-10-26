-- Revision 0005 (subagent telemetry): the statements Alembic's `0005` upgrade runs, in order: new `subagent_runs` columns and their index and foreign key. SQLite cannot add a foreign key to an existing table, so Alembic rebuilds `subagent_runs` in place (the copy, drop and rename below).
-- Statements are separated by a line holding `----`.
----
CREATE TABLE _alembic_tmp_subagent_runs (
	id VARCHAR NOT NULL, 
	session_id VARCHAR NOT NULL, 
	external_agent_id VARCHAR(200) NOT NULL, 
	agent_type VARCHAR(200), 
	attempt_id VARCHAR(36), 
	started_at VARCHAR(32), 
	stopped_at VARCHAR(32), 
	last_message_excerpt TEXT, 
	transcript_path VARCHAR(1000), 
	requested_model VARCHAR(200), 
	model VARCHAR(200), 
	usage TEXT, 
	telemetry TEXT, 
	task_id VARCHAR(36), 
	CONSTRAINT pk_subagent_runs PRIMARY KEY (id), 
	CONSTRAINT fk_subagent_runs_session_id_agent_sessions FOREIGN KEY(session_id) REFERENCES agent_sessions (id) ON DELETE CASCADE, 
	CONSTRAINT uq_subagent_runs_session_id UNIQUE (session_id, external_agent_id), 
	CONSTRAINT fk_subagent_runs_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE SET NULL
)
----
INSERT INTO _alembic_tmp_subagent_runs (id, session_id, external_agent_id, agent_type, attempt_id, started_at, stopped_at, last_message_excerpt, transcript_path) SELECT subagent_runs.id, subagent_runs.session_id, subagent_runs.external_agent_id, subagent_runs.agent_type, subagent_runs.attempt_id, subagent_runs.started_at, subagent_runs.stopped_at, subagent_runs.last_message_excerpt, subagent_runs.transcript_path 
FROM subagent_runs
----
DROP TABLE subagent_runs
----
ALTER TABLE _alembic_tmp_subagent_runs RENAME TO subagent_runs
----
CREATE INDEX ix_subagent_runs_session_id ON subagent_runs (session_id)
----
CREATE INDEX ix_subagent_runs_task_id ON subagent_runs (task_id)
