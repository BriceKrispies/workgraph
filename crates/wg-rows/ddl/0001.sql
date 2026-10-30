-- Revision 0001 (initial schema): the statements Alembic's `0001` upgrade runs, tables first,
-- then indexes, then the append-only triggers. `alembic_version` is the runner's own table.
-- Statements are separated by a line holding `----`.
----
CREATE TABLE agent_attempts (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	number INTEGER NOT NULL,
	status VARCHAR(16) NOT NULL,
	provider VARCHAR(100) NOT NULL,
	adapter VARCHAR(100) NOT NULL,
	agent_id VARCHAR(200),
	model VARCHAR(200),
	session_id VARCHAR(200),
	parent_attempt_id VARCHAR,
	retry_of_attempt_id VARCHAR,
	rerun_of_attempt_id VARCHAR,
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	queued_at VARCHAR(32),
	started_at VARCHAR(32),
	finished_at VARCHAR(32),
	queue_wait_ms INTEGER,
	duration_ms INTEGER,
	exit_reason VARCHAR(100),
	exit_code INTEGER,
	error TEXT,
	summary TEXT,
	input_task_version INTEGER,
	input_task_snapshot TEXT NOT NULL,
	prompt_artifact_id VARCHAR(36),
	config_snapshot TEXT NOT NULL,
	environment TEXT NOT NULL,
	baseline_manifest_id VARCHAR(36),
	outcome_manifest_id VARCHAR(36),
	usage TEXT NOT NULL,
	usage_source VARCHAR(32),
	cost_usd FLOAT,
	cost_source VARCHAR(32),
	telemetry TEXT NOT NULL,
	modified_repository_ids TEXT NOT NULL,
	retries INTEGER NOT NULL,
	cancel_requested BOOLEAN NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_agent_attempts_parent_attempt_id_agent_attempts FOREIGN KEY(parent_attempt_id) REFERENCES agent_attempts (id) ON DELETE SET NULL,
	CONSTRAINT fk_agent_attempts_rerun_of_attempt_id_agent_attempts FOREIGN KEY(rerun_of_attempt_id) REFERENCES agent_attempts (id) ON DELETE SET NULL,
	CONSTRAINT fk_agent_attempts_retry_of_attempt_id_agent_attempts FOREIGN KEY(retry_of_attempt_id) REFERENCES agent_attempts (id) ON DELETE SET NULL,
	CONSTRAINT fk_agent_attempts_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE RESTRICT,
	CONSTRAINT fk_agent_attempts_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_agent_attempts PRIMARY KEY (id),
	CONSTRAINT uq_agent_attempts_task_id UNIQUE (task_id, number)
)
----
CREATE TABLE agent_internal_tasks (
	id VARCHAR NOT NULL,
	session_id VARCHAR NOT NULL,
	external_task_id VARCHAR(200) NOT NULL,
	subject VARCHAR(500) NOT NULL,
	description TEXT,
	status VARCHAR(16) NOT NULL,
	task_id VARCHAR(36),
	external_agent_id VARCHAR(200),
	created_at VARCHAR(32) NOT NULL,
	completed_at VARCHAR(32),
	CONSTRAINT fk_agent_internal_tasks_session_id_agent_sessions FOREIGN KEY(session_id) REFERENCES agent_sessions (id) ON DELETE CASCADE,
	CONSTRAINT pk_agent_internal_tasks PRIMARY KEY (id),
	CONSTRAINT uq_agent_internal_tasks_session_id UNIQUE (session_id, external_task_id)
)
----
CREATE TABLE agent_profiles (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	name VARCHAR(100) NOT NULL,
	adapter VARCHAR(100),
	model VARCHAR(200),
	permissions TEXT NOT NULL,
	enabled BOOLEAN NOT NULL,
	last_seen_at VARCHAR(32),
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_agent_profiles_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_agent_profiles PRIMARY KEY (id),
	CONSTRAINT uq_agent_profiles_workspace_id UNIQUE (workspace_id, name)
)
----
CREATE TABLE agent_sessions (
	id VARCHAR NOT NULL,
	provider VARCHAR(100) NOT NULL,
	external_session_id VARCHAR(200) NOT NULL,
	workspace_id VARCHAR(36),
	attempt_id VARCHAR(36),
	task_id VARCHAR(36),
	binding_source VARCHAR(32) NOT NULL,
	cwd VARCHAR(1000),
	model VARCHAR(200),
	transcript_path VARCHAR(1000),
	started_at VARCHAR(32),
	ended_at VARCHAR(32),
	end_reason VARCHAR(100),
	tool_calls INTEGER NOT NULL,
	tool_failures INTEGER NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT pk_agent_sessions PRIMARY KEY (id),
	CONSTRAINT uq_agent_sessions_provider UNIQUE (provider, external_session_id)
)
----
CREATE TABLE artifacts (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	task_id VARCHAR,
	attempt_id VARCHAR(36),
	kind VARCHAR(32) NOT NULL,
	name VARCHAR(300) NOT NULL,
	media_type VARCHAR(100) NOT NULL,
	path VARCHAR(1000) NOT NULL,
	size_bytes INTEGER NOT NULL,
	sha256 VARCHAR(64) NOT NULL,
	retention VARCHAR(16) NOT NULL,
	purged_at VARCHAR(32),
	metadata TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_artifacts_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE SET NULL,
	CONSTRAINT fk_artifacts_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_artifacts PRIMARY KEY (id)
)
----
CREATE TABLE automation_rules (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	name VARCHAR(200) NOT NULL,
	description TEXT NOT NULL,
	enabled BOOLEAN NOT NULL,
	"trigger" TEXT NOT NULL,
	conditions TEXT NOT NULL,
	actions TEXT NOT NULL,
	retry_max_attempts INTEGER NOT NULL,
	max_depth INTEGER NOT NULL,
	last_scheduled_at VARCHAR(32),
	next_run_at VARCHAR(32),
	created_by VARCHAR(200) NOT NULL,
	version INTEGER NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_automation_rules_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_automation_rules PRIMARY KEY (id),
	CONSTRAINT uq_automation_rules_workspace_id UNIQUE (workspace_id, name)
)
----
CREATE TABLE automation_runs (
	id VARCHAR NOT NULL,
	rule_id VARCHAR NOT NULL,
	workspace_id VARCHAR(36) NOT NULL,
	trigger_type VARCHAR(32) NOT NULL,
	trigger_key VARCHAR(200) NOT NULL,
	trigger_event_id VARCHAR(36),
	depth INTEGER NOT NULL,
	dry_run BOOLEAN NOT NULL,
	status VARCHAR(16) NOT NULL,
	matched BOOLEAN NOT NULL,
	reason TEXT NOT NULL,
	condition_trace TEXT NOT NULL,
	action_results TEXT NOT NULL,
	error TEXT,
	attempts INTEGER NOT NULL,
	correlation_id VARCHAR(36),
	created_at VARCHAR(32) NOT NULL,
	finished_at VARCHAR(32),
	CONSTRAINT fk_automation_runs_rule_id_automation_rules FOREIGN KEY(rule_id) REFERENCES automation_rules (id) ON DELETE CASCADE,
	CONSTRAINT pk_automation_runs PRIMARY KEY (id),
	CONSTRAINT uq_automation_runs_rule_id UNIQUE (rule_id, trigger_key)
)
----
CREATE TABLE change_set_items (
	id VARCHAR NOT NULL,
	change_set_id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	base_sha VARCHAR(64),
	head_sha VARCHAR(64),
	branch VARCHAR(300),
	commit_shas TEXT NOT NULL,
	status VARCHAR(16) NOT NULL,
	detail TEXT,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_change_set_items_change_set_id_change_sets FOREIGN KEY(change_set_id) REFERENCES change_sets (id) ON DELETE CASCADE,
	CONSTRAINT fk_change_set_items_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE RESTRICT,
	CONSTRAINT pk_change_set_items PRIMARY KEY (id),
	CONSTRAINT uq_change_set_items_change_set_id UNIQUE (change_set_id, repository_id)
)
----
CREATE TABLE change_sets (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	title VARCHAR(500) NOT NULL,
	status VARCHAR(32) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_change_sets_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE RESTRICT,
	CONSTRAINT fk_change_sets_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_change_sets PRIMARY KEY (id)
)
----
CREATE TABLE classification_evaluations (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR(36) NOT NULL,
	provider VARCHAR(100) NOT NULL,
	model VARCHAR(200) NOT NULL,
	prompt_version VARCHAR(32) NOT NULL,
	taxonomy_version VARCHAR(32) NOT NULL,
	sample_size INTEGER NOT NULL,
	failures INTEGER NOT NULL,
	accuracy FLOAT,
	macro_f1 FLOAT,
	per_class TEXT NOT NULL,
	confusion TEXT NOT NULL,
	results TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT pk_classification_evaluations PRIMARY KEY (id)
)
----
CREATE TABLE classification_gold_labels (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	task_id VARCHAR,
	title VARCHAR(500) NOT NULL,
	description TEXT NOT NULL,
	category VARCHAR(32) NOT NULL,
	labeled_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_classification_gold_labels_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE SET NULL,
	CONSTRAINT fk_classification_gold_labels_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_classification_gold_labels PRIMARY KEY (id)
)
----
CREATE TABLE classifications (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	source VARCHAR(16) NOT NULL,
	status VARCHAR(16) NOT NULL,
	provider VARCHAR(100),
	model VARCHAR(200),
	prompt_version VARCHAR(32),
	taxonomy_version VARCHAR(32) NOT NULL,
	input_hash VARCHAR(64) NOT NULL,
	category VARCHAR(32),
	subtype VARCHAR(100),
	domain VARCHAR(100),
	affected_systems TEXT NOT NULL,
	complexity VARCHAR(16),
	tags TEXT NOT NULL,
	confidence FLOAT,
	rationale TEXT,
	raw_output TEXT,
	validation_status VARCHAR(16) NOT NULL,
	error TEXT,
	duration_ms INTEGER,
	is_current BOOLEAN NOT NULL,
	confirmed_by VARCHAR(200),
	batch_id VARCHAR(36),
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_classifications_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_classifications PRIMARY KEY (id)
)
----
CREATE TABLE commit_analyses (
	commit_id VARCHAR NOT NULL,
	analyzer_version VARCHAR(32) NOT NULL,
	files_parsed INTEGER NOT NULL,
	files_fallback INTEGER NOT NULL,
	functions_affected INTEGER NOT NULL,
	types_affected INTEGER NOT NULL,
	detail TEXT NOT NULL,
	analyzed_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_commit_analyses_commit_id_commits FOREIGN KEY(commit_id) REFERENCES commits (id) ON DELETE CASCADE,
	CONSTRAINT pk_commit_analyses PRIMARY KEY (commit_id)
)
----
CREATE TABLE commit_equivalences (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	original_commit_id VARCHAR NOT NULL,
	equivalent_commit_id VARCHAR NOT NULL,
	kind VARCHAR(16) NOT NULL,
	evidence TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_commit_equivalences_equivalent_commit_id_commits FOREIGN KEY(equivalent_commit_id) REFERENCES commits (id) ON DELETE CASCADE,
	CONSTRAINT fk_commit_equivalences_original_commit_id_commits FOREIGN KEY(original_commit_id) REFERENCES commits (id) ON DELETE CASCADE,
	CONSTRAINT fk_commit_equivalences_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_commit_equivalences PRIMARY KEY (id),
	CONSTRAINT uq_commit_equivalences_original_commit_id UNIQUE (original_commit_id, equivalent_commit_id)
)
----
CREATE TABLE commit_file_changes (
	id VARCHAR NOT NULL,
	commit_id VARCHAR NOT NULL,
	path VARCHAR(1000) NOT NULL,
	old_path VARCHAR(1000),
	change_type VARCHAR(2) NOT NULL,
	similarity INTEGER,
	insertions INTEGER NOT NULL,
	deletions INTEGER NOT NULL,
	is_binary BOOLEAN NOT NULL,
	language VARCHAR(32),
	file_class VARCHAR(16) NOT NULL,
	module VARCHAR(300) NOT NULL,
	CONSTRAINT fk_commit_file_changes_commit_id_commits FOREIGN KEY(commit_id) REFERENCES commits (id) ON DELETE CASCADE,
	CONSTRAINT pk_commit_file_changes PRIMARY KEY (id)
)
----
CREATE TABLE commit_symbols (
	id VARCHAR NOT NULL,
	file_change_id VARCHAR NOT NULL,
	symbol_kind VARCHAR(32) NOT NULL,
	name VARCHAR(500) NOT NULL,
	change_kind VARCHAR(16) NOT NULL,
	start_line INTEGER,
	end_line INTEGER,
	lines_changed INTEGER NOT NULL,
	CONSTRAINT fk_commit_symbols_file_change_id_commit_file_changes FOREIGN KEY(file_change_id) REFERENCES commit_file_changes (id) ON DELETE CASCADE,
	CONSTRAINT pk_commit_symbols PRIMARY KEY (id)
)
----
CREATE TABLE commit_task_links (
	id VARCHAR NOT NULL,
	commit_id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	source VARCHAR(32) NOT NULL,
	confidence FLOAT NOT NULL,
	evidence TEXT NOT NULL,
	attempt_id VARCHAR(36),
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_commit_task_links_commit_id_commits FOREIGN KEY(commit_id) REFERENCES commits (id) ON DELETE CASCADE,
	CONSTRAINT fk_commit_task_links_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_commit_task_links PRIMARY KEY (id),
	CONSTRAINT uq_commit_task_links_commit_id UNIQUE (commit_id, task_id, source)
)
----
CREATE TABLE commits (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	sha VARCHAR(64) NOT NULL,
	tree_sha VARCHAR(64),
	parents TEXT NOT NULL,
	author_name VARCHAR(300) NOT NULL,
	author_email VARCHAR(300) NOT NULL,
	authored_at VARCHAR(32),
	committer_name VARCHAR(300) NOT NULL,
	committer_email VARCHAR(300) NOT NULL,
	committed_at VARCHAR(32),
	subject VARCHAR(1000) NOT NULL,
	message TEXT NOT NULL,
	trailers TEXT NOT NULL,
	is_merge BOOLEAN NOT NULL,
	patch_id VARCHAR(64),
	insertions INTEGER NOT NULL,
	deletions INTEGER NOT NULL,
	files_changed INTEGER NOT NULL,
	reachable BOOLEAN NOT NULL,
	unreachable_since VARCHAR(32),
	object_available BOOLEAN NOT NULL,
	structure_analyzed BOOLEAN NOT NULL,
	branches TEXT NOT NULL,
	ingested_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_commits_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_commits PRIMARY KEY (id),
	CONSTRAINT uq_commits_repository_id UNIQUE (repository_id, sha)
)
----
CREATE TABLE events (
	seq INTEGER NOT NULL,
	id VARCHAR(36) NOT NULL,
	workspace_id VARCHAR(36),
	event_type VARCHAR(100) NOT NULL,
	entity_type VARCHAR(50) NOT NULL,
	entity_id VARCHAR(36) NOT NULL,
	actor VARCHAR(200) NOT NULL,
	actor_kind VARCHAR(16) NOT NULL,
	occurred_at VARCHAR(32) NOT NULL,
	correlation_id VARCHAR(36) NOT NULL,
	causation_id VARCHAR(36),
	attempt_id VARCHAR(36),
	schema_version INTEGER NOT NULL,
	data TEXT NOT NULL,
	CONSTRAINT pk_events PRIMARY KEY (seq),
	CONSTRAINT uq_events_id UNIQUE (id)
)
----
CREATE TABLE findings (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	kind VARCHAR(64) NOT NULL,
	fingerprint VARCHAR(64) NOT NULL,
	title VARCHAR(500) NOT NULL,
	severity VARCHAR(16) NOT NULL,
	metric_value FLOAT,
	sample_size INTEGER NOT NULL,
	evidence TEXT NOT NULL,
	first_detected_at VARCHAR(32) NOT NULL,
	last_detected_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_findings_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_findings PRIMARY KEY (id),
	CONSTRAINT uq_findings_workspace_id UNIQUE (workspace_id, fingerprint)
)
----
CREATE TABLE git_object_gaps (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	sha VARCHAR(64) NOT NULL,
	context TEXT NOT NULL,
	detected_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_git_object_gaps_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_git_object_gaps PRIMARY KEY (id),
	CONSTRAINT uq_git_object_gaps_repository_id UNIQUE (repository_id, sha)
)
----
CREATE TABLE hook_events (
	id VARCHAR NOT NULL,
	dedupe_key VARCHAR(64) NOT NULL,
	provider VARCHAR(100) NOT NULL,
	hook_event_name VARCHAR(64) NOT NULL,
	external_session_id VARCHAR(200),
	session_id VARCHAR(36),
	attempt_id VARCHAR(36),
	payload TEXT,
	payload_purged_at VARCHAR(32),
	received_at VARCHAR(32) NOT NULL,
	source VARCHAR(16) NOT NULL,
	CONSTRAINT pk_hook_events PRIMARY KEY (id),
	CONSTRAINT uq_hook_events_dedupe_key UNIQUE (dedupe_key)
)
----
CREATE TABLE job_resource_claims (
	job_id VARCHAR NOT NULL,
	resource_key VARCHAR(300) NOT NULL,
	CONSTRAINT fk_job_resource_claims_job_id_jobs FOREIGN KEY(job_id) REFERENCES jobs (id) ON DELETE CASCADE,
	CONSTRAINT pk_job_resource_claims PRIMARY KEY (job_id, resource_key)
)
----
CREATE TABLE job_runs (
	id VARCHAR NOT NULL,
	job_id VARCHAR NOT NULL,
	attempt_number INTEGER NOT NULL,
	worker_id VARCHAR(100) NOT NULL,
	status VARCHAR(16) NOT NULL,
	error TEXT,
	started_at VARCHAR(32) NOT NULL,
	finished_at VARCHAR(32),
	CONSTRAINT fk_job_runs_job_id_jobs FOREIGN KEY(job_id) REFERENCES jobs (id) ON DELETE CASCADE,
	CONSTRAINT pk_job_runs PRIMARY KEY (id)
)
----
CREATE TABLE jobs (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR(36),
	kind VARCHAR(64) NOT NULL,
	status VARCHAR(16) NOT NULL,
	priority INTEGER NOT NULL,
	payload TEXT NOT NULL,
	result TEXT,
	run_at VARCHAR(32) NOT NULL,
	attempts INTEGER NOT NULL,
	max_attempts INTEGER NOT NULL,
	backoff_base_seconds FLOAT NOT NULL,
	timeout_seconds FLOAT,
	lease_owner VARCHAR(100),
	lease_expires_at VARCHAR(32),
	heartbeat_at VARCHAR(32),
	started_at VARCHAR(32),
	finished_at VARCHAR(32),
	cancel_requested BOOLEAN NOT NULL,
	idempotency_key VARCHAR(300),
	resource_keys TEXT NOT NULL,
	last_error TEXT,
	error_kind VARCHAR(64),
	correlation_id VARCHAR(36),
	causation_id VARCHAR(36),
	task_id VARCHAR(36),
	attempt_id VARCHAR(36),
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT pk_jobs PRIMARY KEY (id),
	CONSTRAINT uq_jobs_idempotency_key UNIQUE (idempotency_key)
)
----
CREATE TABLE managed_worktrees (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	source_checkout_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	path VARCHAR(1000) NOT NULL,
	branch VARCHAR(300) NOT NULL,
	base_sha VARCHAR(64) NOT NULL,
	status VARCHAR(16) NOT NULL,
	last_error TEXT,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	removed_at VARCHAR(32),
	CONSTRAINT fk_managed_worktrees_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE RESTRICT,
	CONSTRAINT fk_managed_worktrees_source_checkout_id_repository_checkouts FOREIGN KEY(source_checkout_id) REFERENCES repository_checkouts (id) ON DELETE RESTRICT,
	CONSTRAINT fk_managed_worktrees_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_managed_worktrees PRIMARY KEY (id),
	CONSTRAINT uq_managed_worktrees_path UNIQUE (path)
)
----
CREATE TABLE notifications (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	level VARCHAR(16) NOT NULL,
	title VARCHAR(300) NOT NULL,
	body TEXT NOT NULL,
	source VARCHAR(200) NOT NULL,
	data TEXT NOT NULL,
	read_at VARCHAR(32),
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_notifications_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_notifications PRIMARY KEY (id)
)
----
CREATE TABLE project_repositories (
	project_id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	role VARCHAR(32) NOT NULL,
	access_mode VARCHAR(16),
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_project_repositories_project_id_projects FOREIGN KEY(project_id) REFERENCES projects (id) ON DELETE CASCADE,
	CONSTRAINT fk_project_repositories_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_project_repositories PRIMARY KEY (project_id, repository_id)
)
----
CREATE TABLE projects (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	slug VARCHAR(64) NOT NULL,
	name VARCHAR(200) NOT NULL,
	description TEXT NOT NULL,
	status VARCHAR(16) NOT NULL,
	settings TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_projects_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_projects PRIMARY KEY (id),
	CONSTRAINT uq_projects_workspace_id UNIQUE (workspace_id, slug)
)
----
CREATE TABLE reports (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	kind VARCHAR(32) NOT NULL,
	title VARCHAR(300) NOT NULL,
	scope TEXT NOT NULL,
	content_markdown TEXT NOT NULL,
	data TEXT NOT NULL,
	narrative_source VARCHAR(200) NOT NULL,
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_reports_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_reports PRIMARY KEY (id)
)
----
CREATE TABLE repositories (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	name VARCHAR(200) NOT NULL,
	description TEXT NOT NULL,
	identity_fingerprint VARCHAR(400),
	default_branch VARCHAR(200),
	tracked_branches TEXT NOT NULL,
	access_mode VARCHAR(16) NOT NULL,
	health_status VARCHAR(16) NOT NULL,
	health_detail TEXT NOT NULL,
	health_checked_at VARCHAR(32),
	is_shallow BOOLEAN NOT NULL,
	last_scanned_at VARCHAR(32),
	archived_at VARCHAR(32),
	metadata TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_repositories_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_repositories PRIMARY KEY (id),
	CONSTRAINT uq_repositories_workspace_id UNIQUE (workspace_id, name)
)
----
CREATE TABLE repository_checkouts (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	path VARCHAR(1000) NOT NULL,
	kind VARCHAR(32) NOT NULL,
	is_primary BOOLEAN NOT NULL,
	access_mode VARCHAR(16),
	available BOOLEAN NOT NULL,
	last_seen_at VARCHAR(32),
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_repository_checkouts_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_repository_checkouts PRIMARY KEY (id),
	CONSTRAINT uq_repository_checkouts_repository_id UNIQUE (repository_id, path)
)
----
CREATE TABLE repository_dependencies (
	id VARCHAR NOT NULL,
	from_repository_id VARCHAR NOT NULL,
	to_repository_id VARCHAR NOT NULL,
	kind VARCHAR(32) NOT NULL,
	description TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT ck_repository_dependencies_no_self CHECK (from_repository_id <> to_repository_id),
	CONSTRAINT fk_repository_dependencies_from_repository_id_repositories FOREIGN KEY(from_repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT fk_repository_dependencies_to_repository_id_repositories FOREIGN KEY(to_repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_repository_dependencies PRIMARY KEY (id),
	CONSTRAINT uq_repository_dependencies_from_repository_id UNIQUE (from_repository_id, to_repository_id)
)
----
CREATE TABLE repository_refs (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	name VARCHAR(500) NOT NULL,
	sha VARCHAR(64) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_repository_refs_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_repository_refs PRIMARY KEY (id),
	CONSTRAINT uq_repository_refs_repository_id UNIQUE (repository_id, name)
)
----
CREATE TABLE repository_remotes (
	id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	name VARCHAR(200) NOT NULL,
	url VARCHAR(1000) NOT NULL,
	normalized_url VARCHAR(1000) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_repository_remotes_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT pk_repository_remotes PRIMARY KEY (id),
	CONSTRAINT uq_repository_remotes_repository_id UNIQUE (repository_id, name)
)
----
CREATE TABLE resource_leases (
	resource_key VARCHAR(300) NOT NULL,
	holder VARCHAR(200) NOT NULL,
	purpose VARCHAR(200) NOT NULL,
	acquired_at VARCHAR(32) NOT NULL,
	expires_at VARCHAR(32),
	CONSTRAINT pk_resource_leases PRIMARY KEY (resource_key)
)
----
CREATE TABLE revision_manifests (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	kind VARCHAR(32) NOT NULL,
	entries TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_revision_manifests_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_revision_manifests PRIMARY KEY (id)
)
----
CREATE TABLE session_bindings (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	provider VARCHAR(100) NOT NULL,
	external_session_id VARCHAR(200),
	cwd_prefix VARCHAR(1000),
	active BOOLEAN NOT NULL,
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_session_bindings_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT fk_session_bindings_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_session_bindings PRIMARY KEY (id)
)
----
CREATE TABLE subagent_runs (
	id VARCHAR NOT NULL,
	session_id VARCHAR NOT NULL,
	external_agent_id VARCHAR(200) NOT NULL,
	agent_type VARCHAR(200),
	attempt_id VARCHAR(36),
	started_at VARCHAR(32),
	stopped_at VARCHAR(32),
	last_message_excerpt TEXT,
	transcript_path VARCHAR(1000),
	CONSTRAINT fk_subagent_runs_session_id_agent_sessions FOREIGN KEY(session_id) REFERENCES agent_sessions (id) ON DELETE CASCADE,
	CONSTRAINT pk_subagent_runs PRIMARY KEY (id),
	CONSTRAINT uq_subagent_runs_session_id UNIQUE (session_id, external_agent_id)
)
----
CREATE TABLE system_cursors (
	name VARCHAR(100) NOT NULL,
	value INTEGER NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT pk_system_cursors PRIMARY KEY (name)
)
----
CREATE TABLE task_acceptances (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	verification_run_id VARCHAR(36),
	decision VARCHAR(16) NOT NULL,
	mode VARCHAR(16) NOT NULL,
	reason TEXT NOT NULL,
	actor VARCHAR(200) NOT NULL,
	manifest_id VARCHAR(36),
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_task_acceptances_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_acceptances PRIMARY KEY (id)
)
----
CREATE TABLE task_change_surfaces (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	base_sha VARCHAR(64) NOT NULL,
	final_sha VARCHAR(64) NOT NULL,
	files_changed INTEGER NOT NULL,
	insertions INTEGER NOT NULL,
	deletions INTEGER NOT NULL,
	renames INTEGER NOT NULL,
	binary_files INTEGER NOT NULL,
	functions_affected INTEGER NOT NULL,
	types_affected INTEGER NOT NULL,
	detail TEXT NOT NULL,
	computed_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_task_change_surfaces_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT fk_task_change_surfaces_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_change_surfaces PRIMARY KEY (id),
	CONSTRAINT uq_task_change_surfaces_task_id UNIQUE (task_id, repository_id, attempt_id)
)
----
CREATE TABLE task_comments (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	author VARCHAR(200) NOT NULL,
	kind VARCHAR(16) NOT NULL,
	body TEXT NOT NULL,
	data TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_task_comments_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_comments PRIMARY KEY (id)
)
----
CREATE TABLE task_dependencies (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	depends_on_id VARCHAR NOT NULL,
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT ck_task_dependencies_no_self CHECK (task_id <> depends_on_id),
	CONSTRAINT fk_task_dependencies_depends_on_id_tasks FOREIGN KEY(depends_on_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT fk_task_dependencies_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_dependencies PRIMARY KEY (id),
	CONSTRAINT uq_task_dependencies_task_id UNIQUE (task_id, depends_on_id)
)
----
CREATE TABLE task_embeddings (
	id VARCHAR NOT NULL,
	task_id VARCHAR NOT NULL,
	provider VARCHAR(100) NOT NULL,
	model VARCHAR(200) NOT NULL,
	dimensions INTEGER NOT NULL,
	vector BLOB NOT NULL,
	input_hash VARCHAR(64) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_task_embeddings_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_embeddings PRIMARY KEY (id),
	CONSTRAINT uq_task_embeddings_task_id UNIQUE (task_id, provider, model)
)
----
CREATE VIRTUAL TABLE task_fts USING fts5 (
	task_id UNINDEXED,
	workspace_id UNINDEXED,
	key,
	title,
	description,
	comments,
	tags,
	summary,
	tokenize='porter unicode61'
)
----
CREATE TABLE task_projects (
	task_id VARCHAR NOT NULL,
	project_id VARCHAR NOT NULL,
	CONSTRAINT fk_task_projects_project_id_projects FOREIGN KEY(project_id) REFERENCES projects (id) ON DELETE CASCADE,
	CONSTRAINT fk_task_projects_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_projects PRIMARY KEY (task_id, project_id)
)
----
CREATE TABLE task_relations (
	id VARCHAR NOT NULL,
	source_task_id VARCHAR NOT NULL,
	target_task_id VARCHAR NOT NULL,
	relation_type VARCHAR(32) NOT NULL,
	created_by VARCHAR(200) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT ck_task_relations_no_self CHECK (source_task_id <> target_task_id),
	CONSTRAINT fk_task_relations_source_task_id_tasks FOREIGN KEY(source_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT fk_task_relations_target_task_id_tasks FOREIGN KEY(target_task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_relations PRIMARY KEY (id),
	CONSTRAINT uq_task_relations_source_task_id UNIQUE (source_task_id, target_task_id, relation_type)
)
----
CREATE TABLE task_repositories (
	task_id VARCHAR NOT NULL,
	repository_id VARCHAR NOT NULL,
	access VARCHAR(16) NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_task_repositories_repository_id_repositories FOREIGN KEY(repository_id) REFERENCES repositories (id) ON DELETE CASCADE,
	CONSTRAINT fk_task_repositories_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_repositories PRIMARY KEY (task_id, repository_id)
)
----
CREATE TABLE task_tags (
	task_id VARCHAR NOT NULL,
	tag VARCHAR(100) NOT NULL,
	CONSTRAINT fk_task_tags_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_task_tags PRIMARY KEY (task_id, tag)
)
----
CREATE TABLE tasks (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	number INTEGER NOT NULL,
	"key" VARCHAR(32) NOT NULL,
	title VARCHAR(500) NOT NULL,
	description TEXT NOT NULL,
	description_hash VARCHAR(64) NOT NULL,
	acceptance_criteria TEXT NOT NULL,
	status VARCHAR(32) NOT NULL,
	priority VARCHAR(16) NOT NULL,
	category VARCHAR(32),
	parent_id VARCHAR,
	assignee VARCHAR(200),
	assigned_agent VARCHAR(100),
	claimed_by VARCHAR(200),
	claim_expires_at VARCHAR(32),
	auto_execute BOOLEAN NOT NULL,
	estimated_effort TEXT,
	external_refs TEXT NOT NULL,
	metadata TEXT NOT NULL,
	due_at VARCHAR(32),
	scheduled_at VARCHAR(32),
	created_by VARCHAR(200) NOT NULL,
	version INTEGER NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	started_at VARCHAR(32),
	completed_at VARCHAR(32),
	accepted_at VARCHAR(32),
	archived_at VARCHAR(32),
	current_classification_id VARCHAR(36),
	CONSTRAINT ck_tasks_no_self_parent CHECK (parent_id IS NULL OR parent_id <> id),
	CONSTRAINT fk_tasks_parent_id_tasks FOREIGN KEY(parent_id) REFERENCES tasks (id) ON DELETE RESTRICT,
	CONSTRAINT fk_tasks_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE RESTRICT,
	CONSTRAINT pk_tasks PRIMARY KEY (id),
	CONSTRAINT uq_tasks_workspace_id UNIQUE (workspace_id, "key"),
	CONSTRAINT uq_tasks_workspace_id UNIQUE (workspace_id, number)
)
----
CREATE TABLE tool_events (
	id VARCHAR NOT NULL,
	hook_event_id VARCHAR NOT NULL,
	session_id VARCHAR(36),
	attempt_id VARCHAR(36),
	external_agent_id VARCHAR(200),
	phase VARCHAR(16) NOT NULL,
	tool_name VARCHAR(200) NOT NULL,
	tool_use_id VARCHAR(200),
	duration_ms INTEGER,
	error_excerpt TEXT,
	summary TEXT NOT NULL,
	occurred_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_tool_events_hook_event_id_hook_events FOREIGN KEY(hook_event_id) REFERENCES hook_events (id) ON DELETE CASCADE,
	CONSTRAINT pk_tool_events PRIMARY KEY (id),
	CONSTRAINT uq_tool_events_hook_event_id UNIQUE (hook_event_id)
)
----
CREATE TABLE verification_check_results (
	id VARCHAR NOT NULL,
	run_id VARCHAR NOT NULL,
	name VARCHAR(200) NOT NULL,
	kind VARCHAR(16) NOT NULL,
	required BOOLEAN NOT NULL,
	repository_id VARCHAR(36),
	status VARCHAR(16) NOT NULL,
	exit_code INTEGER,
	duration_ms INTEGER,
	timed_out BOOLEAN NOT NULL,
	log_artifact_id VARCHAR(36),
	output_excerpt TEXT NOT NULL,
	failure_signatures TEXT NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_verification_check_results_run_id_verification_runs FOREIGN KEY(run_id) REFERENCES verification_runs (id) ON DELETE CASCADE,
	CONSTRAINT pk_verification_check_results PRIMARY KEY (id)
)
----
CREATE TABLE verification_configs (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR NOT NULL,
	scope_type VARCHAR(16) NOT NULL,
	scope_id VARCHAR(36) NOT NULL,
	name VARCHAR(200) NOT NULL,
	checks TEXT NOT NULL,
	enabled BOOLEAN NOT NULL,
	approved_by VARCHAR(200) NOT NULL,
	version INTEGER NOT NULL,
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_verification_configs_workspace_id_workspaces FOREIGN KEY(workspace_id) REFERENCES workspaces (id) ON DELETE CASCADE,
	CONSTRAINT pk_verification_configs PRIMARY KEY (id),
	CONSTRAINT uq_verification_configs_workspace_id UNIQUE (workspace_id, scope_type, scope_id, name)
)
----
CREATE TABLE verification_runs (
	id VARCHAR NOT NULL,
	workspace_id VARCHAR(36) NOT NULL,
	task_id VARCHAR NOT NULL,
	attempt_id VARCHAR(36),
	manifest_id VARCHAR(36),
	config_snapshot TEXT NOT NULL,
	status VARCHAR(16) NOT NULL,
	required_passed BOOLEAN NOT NULL,
	triggered_by VARCHAR(200) NOT NULL,
	started_at VARCHAR(32),
	finished_at VARCHAR(32),
	created_at VARCHAR(32) NOT NULL,
	CONSTRAINT fk_verification_runs_task_id_tasks FOREIGN KEY(task_id) REFERENCES tasks (id) ON DELETE CASCADE,
	CONSTRAINT pk_verification_runs PRIMARY KEY (id)
)
----
CREATE TABLE workers (
	id VARCHAR(100) NOT NULL,
	hostname VARCHAR(200) NOT NULL,
	pid INTEGER NOT NULL,
	status VARCHAR(16) NOT NULL,
	concurrency INTEGER NOT NULL,
	kinds TEXT NOT NULL,
	current_jobs TEXT NOT NULL,
	jobs_completed INTEGER NOT NULL,
	jobs_failed INTEGER NOT NULL,
	started_at VARCHAR(32) NOT NULL,
	heartbeat_at VARCHAR(32) NOT NULL,
	stopped_at VARCHAR(32),
	CONSTRAINT pk_workers PRIMARY KEY (id)
)
----
CREATE TABLE workspaces (
	id VARCHAR NOT NULL,
	slug VARCHAR(64) NOT NULL,
	name VARCHAR(200) NOT NULL,
	description TEXT NOT NULL,
	key_prefix VARCHAR(12) NOT NULL,
	next_task_number INTEGER NOT NULL,
	settings TEXT NOT NULL,
	archived_at VARCHAR(32),
	created_at VARCHAR(32) NOT NULL,
	updated_at VARCHAR(32) NOT NULL,
	CONSTRAINT pk_workspaces PRIMARY KEY (id),
	CONSTRAINT uq_workspaces_slug UNIQUE (slug)
)
----
CREATE INDEX ix_agent_attempts_session_id ON agent_attempts (session_id)
----
CREATE INDEX ix_agent_attempts_task_id ON agent_attempts (task_id)
----
CREATE INDEX ix_agent_internal_tasks_session_id ON agent_internal_tasks (session_id)
----
CREATE INDEX ix_agent_profiles_workspace_id ON agent_profiles (workspace_id)
----
CREATE INDEX ix_agent_sessions_attempt_id ON agent_sessions (attempt_id)
----
CREATE INDEX ix_agent_sessions_task_id ON agent_sessions (task_id)
----
CREATE INDEX ix_agent_sessions_workspace_id ON agent_sessions (workspace_id)
----
CREATE INDEX ix_artifacts_attempt_id ON artifacts (attempt_id)
----
CREATE INDEX ix_artifacts_task_id ON artifacts (task_id)
----
CREATE INDEX ix_artifacts_workspace_id ON artifacts (workspace_id)
----
CREATE INDEX ix_attempts_ws_status ON agent_attempts (workspace_id, status)
----
CREATE INDEX ix_automation_rules_workspace_id ON automation_rules (workspace_id)
----
CREATE INDEX ix_automation_runs_rule_id ON automation_runs (rule_id)
----
CREATE INDEX ix_automation_runs_workspace_id ON automation_runs (workspace_id)
----
CREATE INDEX ix_change_set_items_change_set_id ON change_set_items (change_set_id)
----
CREATE INDEX ix_change_sets_attempt_id ON change_sets (attempt_id)
----
CREATE INDEX ix_change_sets_task_id ON change_sets (task_id)
----
CREATE INDEX ix_classification_evaluations_workspace_id ON classification_evaluations (workspace_id)
----
CREATE INDEX ix_classification_gold_labels_workspace_id ON classification_gold_labels (workspace_id)
----
CREATE INDEX ix_classifications_task ON classifications (task_id, created_at)
----
CREATE INDEX ix_commit_equivalences_repository_id ON commit_equivalences (repository_id)
----
CREATE INDEX ix_commit_file_changes_commit_id ON commit_file_changes (commit_id)
----
CREATE INDEX ix_commit_file_changes_path ON commit_file_changes (path)
----
CREATE INDEX ix_commit_symbols_file_change_id ON commit_symbols (file_change_id)
----
CREATE INDEX ix_commit_task_links_commit_id ON commit_task_links (commit_id)
----
CREATE INDEX ix_commit_task_links_task_id ON commit_task_links (task_id)
----
CREATE INDEX ix_commits_patch_id ON commits (patch_id)
----
CREATE INDEX ix_commits_repo_time ON commits (repository_id, committed_at)
----
CREATE INDEX ix_events_attempt_id ON events (attempt_id)
----
CREATE INDEX ix_events_correlation_id ON events (correlation_id)
----
CREATE INDEX ix_events_entity ON events (entity_type, entity_id)
----
CREATE INDEX ix_events_event_type ON events (event_type)
----
CREATE INDEX ix_events_ws_seq ON events (workspace_id, seq)
----
CREATE INDEX ix_findings_workspace_id ON findings (workspace_id)
----
CREATE INDEX ix_hook_events_attempt_id ON hook_events (attempt_id)
----
CREATE INDEX ix_hook_events_external_session_id ON hook_events (external_session_id)
----
CREATE INDEX ix_hook_events_hook_event_name ON hook_events (hook_event_name)
----
CREATE INDEX ix_job_resource_claims_resource_key ON job_resource_claims (resource_key)
----
CREATE INDEX ix_job_runs_job_id ON job_runs (job_id)
----
CREATE INDEX ix_jobs_attempt_id ON jobs (attempt_id)
----
CREATE INDEX ix_jobs_claim ON jobs (status, run_at, priority)
----
CREATE INDEX ix_jobs_kind ON jobs (kind)
----
CREATE INDEX ix_jobs_task_id ON jobs (task_id)
----
CREATE INDEX ix_jobs_workspace_id ON jobs (workspace_id)
----
CREATE INDEX ix_managed_worktrees_attempt_id ON managed_worktrees (attempt_id)
----
CREATE INDEX ix_managed_worktrees_repository_id ON managed_worktrees (repository_id)
----
CREATE INDEX ix_managed_worktrees_source_checkout_id ON managed_worktrees (source_checkout_id)
----
CREATE INDEX ix_notifications_workspace_id ON notifications (workspace_id)
----
CREATE INDEX ix_project_repositories_repository_id ON project_repositories (repository_id)
----
CREATE INDEX ix_projects_workspace_id ON projects (workspace_id)
----
CREATE INDEX ix_reports_workspace_id ON reports (workspace_id)
----
CREATE INDEX ix_repositories_identity_fingerprint ON repositories (identity_fingerprint)
----
CREATE INDEX ix_repositories_workspace_id ON repositories (workspace_id)
----
CREATE INDEX ix_repository_checkouts_path ON repository_checkouts (path)
----
CREATE INDEX ix_repository_checkouts_repository_id ON repository_checkouts (repository_id)
----
CREATE INDEX ix_repository_refs_repository_id ON repository_refs (repository_id)
----
CREATE INDEX ix_repository_remotes_normalized_url ON repository_remotes (normalized_url)
----
CREATE INDEX ix_repository_remotes_repository_id ON repository_remotes (repository_id)
----
CREATE INDEX ix_revision_manifests_attempt_id ON revision_manifests (attempt_id)
----
CREATE INDEX ix_revision_manifests_workspace_id ON revision_manifests (workspace_id)
----
CREATE INDEX ix_session_bindings_external_session_id ON session_bindings (external_session_id)
----
CREATE INDEX ix_session_bindings_task_id ON session_bindings (task_id)
----
CREATE INDEX ix_subagent_runs_session_id ON subagent_runs (session_id)
----
CREATE INDEX ix_task_acceptances_task_id ON task_acceptances (task_id)
----
CREATE INDEX ix_task_change_surfaces_task_id ON task_change_surfaces (task_id)
----
CREATE INDEX ix_task_comments_attempt_id ON task_comments (attempt_id)
----
CREATE INDEX ix_task_comments_task_id ON task_comments (task_id)
----
CREATE INDEX ix_task_dependencies_depends_on_id ON task_dependencies (depends_on_id)
----
CREATE INDEX ix_task_dependencies_task_id ON task_dependencies (task_id)
----
CREATE INDEX ix_task_embeddings_task_id ON task_embeddings (task_id)
----
CREATE INDEX ix_task_projects_project_id ON task_projects (project_id)
----
CREATE INDEX ix_task_relations_source_task_id ON task_relations (source_task_id)
----
CREATE INDEX ix_task_relations_target_task_id ON task_relations (target_task_id)
----
CREATE INDEX ix_task_repositories_repository_id ON task_repositories (repository_id)
----
CREATE INDEX ix_task_tags_tag ON task_tags (tag)
----
CREATE INDEX ix_tasks_assignee ON tasks (assignee)
----
CREATE INDEX ix_tasks_category ON tasks (category)
----
CREATE INDEX ix_tasks_parent_id ON tasks (parent_id)
----
CREATE INDEX ix_tasks_ws_status ON tasks (workspace_id, status)
----
CREATE INDEX ix_tool_events_attempt_id ON tool_events (attempt_id)
----
CREATE INDEX ix_tool_events_session_id ON tool_events (session_id)
----
CREATE INDEX ix_tool_events_tool_name ON tool_events (tool_name)
----
CREATE INDEX ix_verification_check_results_run_id ON verification_check_results (run_id)
----
CREATE INDEX ix_verification_configs_workspace_id ON verification_configs (workspace_id)
----
CREATE INDEX ix_verification_runs_attempt_id ON verification_runs (attempt_id)
----
CREATE INDEX ix_verification_runs_task_id ON verification_runs (task_id)
----
CREATE INDEX ix_verification_runs_workspace_id ON verification_runs (workspace_id)
----
CREATE TRIGGER events_no_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT, 'events are append-only'); END
----
CREATE TRIGGER events_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT, 'events are append-only'); END
