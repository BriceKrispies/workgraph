//! Row codecs: one [`TableRow`] implementation per model table.
//!
//! Each table is declared once, in the [`table_rows!`](crate::rowmap) list at the end of this
//! file: its fields in the column order of `contracts/schema-0011.sql`, each with the codec that
//! encodes and decodes the column (see `rowmap.rs` for the codecs). The declaration gives the
//! column list, `values` (the bound parameters in that order), `decode` (a row read back by
//! column name) and the primary key. The model's entity structs are the rows (their fields are
//! the columns), so the codecs add no intermediate type.

use wg_model::entity::*;
use wg_model::{RowKey, Table};
use wg_types::WgError;

use crate::codec::{Cols, Raw};

/// A model value that is one row of one table.
pub trait TableRow: Sized {
    const TABLE: Table;

    /// The table's columns, in schema order.
    const COLUMNS: &'static [&'static str];

    /// The primary-key columns, in key order.
    const PRIMARY_KEY: &'static [&'static str];

    /// The primary-key values of `key`, in the order of [`Self::PRIMARY_KEY`]; empty when `key`
    /// is another table's.
    fn key_values(key: &RowKey) -> Vec<Raw>;

    /// The bound parameters, in the order of [`Self::COLUMNS`].
    fn values(&self) -> Result<Vec<Raw>, WgError>;

    /// Reads the row from a result that selects at least [`columns`].
    fn decode(cols: &Cols<'_>) -> Result<Self, WgError>;

    /// Decodes a row whose values are in [`columns`] order.
    fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        Self::decode(&Cols::new(columns(Self::TABLE), values))
    }
}

table_rows! {
    Workspace key(id: id) {
        id: id,
        slug: text,
        name: text,
        description: text,
        key_prefix: text,
        next_task_number: int,
        settings: object,
        archived_at: opt_ts,
        created_at: ts,
        updated_at: ts,
    }

    Project key(id: id) {
        id: id,
        workspace_id: id,
        slug: text,
        name: text,
        description: text,
        status: text,
        settings: object,
        created_at: ts,
        updated_at: ts,
    }

    ProjectRepository key{project_id: id, repository_id: id} {
        project_id: id,
        repository_id: id,
        role: text,
        access_mode: opt_text,
        created_at: ts,
    }

    Repository key(id: id) {
        id: id,
        workspace_id: id,
        name: text,
        description: text,
        identity_fingerprint: opt_text,
        default_branch: opt_text,
        tracked_branches: strings,
        access_mode: parsed,
        health_status: text,
        health_detail: object,
        health_checked_at: opt_ts,
        is_shallow: bool,
        last_scanned_at: opt_ts,
        archived_at: opt_ts,
        metadata: object,
        created_at: ts,
        updated_at: ts,
    }

    RepositoryCheckout key(id: id) {
        id: id,
        repository_id: id,
        path: text,
        kind: text,
        is_primary: bool,
        access_mode: opt_text,
        available: bool,
        last_seen_at: opt_ts,
        created_at: ts,
        updated_at: ts,
    }

    RepositoryRemote key(id: id) {
        id: id,
        repository_id: id,
        name: text,
        url: text,
        normalized_url: text,
        created_at: ts,
        updated_at: ts,
    }

    RepositoryRef key(id: id) {
        id: id,
        repository_id: id,
        name: text,
        sha: text,
        updated_at: ts,
    }

    RepositoryDependency key(id: id) {
        id: id,
        from_repository_id: id,
        to_repository_id: id,
        kind: text,
        description: text,
        created_at: ts,
    }

    Task key(id: id) {
        id: id,
        workspace_id: id,
        number: int,
        key: task_key,
        title: text,
        description: text,
        description_hash: text,
        acceptance_criteria: strings,
        status: parsed,
        priority: parsed,
        category: opt_text,
        parent_id: opt_id,
        assignee: opt_text,
        assigned_agent: opt_text,
        claimed_by: opt_text,
        claim_expires_at: opt_ts,
        auto_execute: bool,
        estimated_effort: opt_object,
        metadata: object,
        due_at: opt_ts,
        scheduled_at: opt_ts,
        created_by: text,
        version: int,
        created_at: ts,
        updated_at: ts,
        started_at: opt_ts,
        completed_at: opt_ts,
        accepted_at: opt_ts,
        archived_at: opt_ts,
        current_classification_id: opt_id,
    }

    TaskFile key{task_id: id, position: int} {
        task_id: id,
        position: int,
        path: text,
        anchor: opt_text,
        note: text,
    }

    TaskTag key{task_id: id, tag: text} {
        task_id: id,
        tag: text,
    }

    TaskLink key(id: id) {
        id: id,
        task_id: id,
        kind: text,
        provider: text,
        url: text,
        normalized_url: text,
        repo_full_name: text,
        number: opt_int,
        repository_id: opt_id,
        sources: strings,
        title: text,
        created_by: text,
        created_at: ts,
        updated_at: ts,
    }

    TaskComment key(id: id) {
        id: id,
        task_id: id,
        attempt_id: opt_id,
        author: text,
        kind: parsed,
        body: text,
        data: object,
        created_at: ts,
    }

    TaskDependency key(id: id) {
        id: id,
        task_id: id,
        depends_on_id: id,
        created_by: text,
        created_at: ts,
    }

    TaskRelation key(id: id) {
        id: id,
        source_task_id: id,
        target_task_id: id,
        relation_type: parsed,
        created_by: text,
        created_at: ts,
    }

    TaskProject key{task_id: id, project_id: id} {
        task_id: id,
        project_id: id,
    }

    TaskRepository key{task_id: id, repository_id: id} {
        task_id: id,
        repository_id: id,
        access: text,
        created_at: ts,
    }

    ExternalItem key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        connection: text,
        external_id: text,
        external_key: text,
        url: text,
        tracker_status: text,
        tracker_status_category: text,
        tracker_resolution: opt_text,
        remote_updated_at: opt_ts,
        snapshot: object,
        snapshot_hash: text,
        synced_at: opt_ts,
        missing_since: opt_ts,
        created_at: ts,
        updated_at: ts,
    }

    TaskDecision key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        question: text,
        options: decision_options,
        status: text,
        chosen_option_id: opt_text,
        other_text: opt_text,
        note: opt_text,
        decided_by: opt_text,
        decided_at: opt_ts,
        created_by: text,
        created_at: ts,
        updated_at: ts,
        version: int,
    }

    Commit key(id: id) {
        id: id,
        repository_id: id,
        sha: text,
        tree_sha: opt_text,
        parents: strings,
        author_name: text,
        author_email: text,
        authored_at: opt_ts,
        committer_name: text,
        committer_email: text,
        committed_at: opt_ts,
        subject: text,
        message: text,
        trailers: objects,
        is_merge: bool,
        patch_id: opt_text,
        insertions: int,
        deletions: int,
        files_changed: int,
        reachable: bool,
        unreachable_since: opt_ts,
        object_available: bool,
        structure_analyzed: bool,
        branches: strings,
        ingested_at: ts,
    }

    CommitFileChange key(id: id) {
        id: id,
        commit_id: id,
        path: text,
        old_path: opt_text,
        change_type: text,
        similarity: opt_int,
        insertions: int,
        deletions: int,
        is_binary: bool,
        language: opt_text,
        file_class: text,
        module: text,
    }

    CommitSymbol key(id: id) {
        id: id,
        file_change_id: id,
        symbol_kind: text,
        name: text,
        change_kind: text,
        start_line: opt_int,
        end_line: opt_int,
        lines_changed: int,
    }

    CommitAnalysis key(commit_id: id) {
        commit_id: id,
        analyzer_version: text,
        files_parsed: int,
        files_fallback: int,
        functions_affected: int,
        types_affected: int,
        detail: object,
        analyzed_at: ts,
    }

    CommitEquivalence key(id: id) {
        id: id,
        repository_id: id,
        original_commit_id: id,
        equivalent_commit_id: id,
        kind: text,
        evidence: object,
        created_at: ts,
    }

    CommitTaskLink key(id: id) {
        id: id,
        commit_id: id,
        task_id: id,
        source: text,
        confidence: real,
        evidence: object,
        attempt_id: opt_id,
        created_by: text,
        created_at: ts,
    }

    GitObjectGap key(id: id) {
        id: id,
        repository_id: id,
        sha: text,
        context: text,
        detected_at: ts,
    }

    TaskChangeSurface key(id: id) {
        id: id,
        task_id: id,
        repository_id: id,
        attempt_id: opt_id,
        base_sha: text,
        final_sha: text,
        files_changed: int,
        insertions: int,
        deletions: int,
        renames: int,
        binary_files: int,
        functions_affected: int,
        types_affected: int,
        detail: object,
        computed_at: ts,
        updated_at: ts,
    }

    VerificationConfig key(id: id) {
        id: id,
        workspace_id: id,
        scope_type: text,
        scope_id: id,
        name: text,
        checks: objects,
        enabled: bool,
        approved_by: text,
        version: int,
        created_at: ts,
        updated_at: ts,
    }

    VerificationRun key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        attempt_id: opt_id,
        manifest_id: opt_id,
        config_snapshot: objects,
        status: text,
        required_passed: bool,
        triggered_by: text,
        started_at: opt_ts,
        finished_at: opt_ts,
        created_at: ts,
    }

    VerificationCheckResult key(id: id) {
        id: id,
        run_id: id,
        name: text,
        kind: text,
        required: bool,
        repository_id: opt_id,
        status: text,
        exit_code: opt_int,
        duration_ms: opt_int,
        timed_out: bool,
        log_artifact_id: opt_id,
        output_excerpt: text,
        failure_signatures: strings,
        created_at: ts,
    }

    TaskAcceptance key(id: id) {
        id: id,
        task_id: id,
        attempt_id: opt_id,
        verification_run_id: opt_id,
        decision: text,
        mode: text,
        reason: text,
        actor: text,
        manifest_id: opt_id,
        created_at: ts,
    }

    Classification key(id: id) {
        id: id,
        task_id: id,
        source: text,
        status: text,
        provider: opt_text,
        model: opt_text,
        prompt_version: opt_text,
        taxonomy_version: text,
        input_hash: text,
        category: opt_text,
        subtype: opt_text,
        domain: opt_text,
        affected_systems: strings,
        complexity: opt_text,
        tags: strings,
        confidence: opt_real,
        rationale: opt_text,
        raw_output: opt_object,
        validation_status: text,
        error: opt_text,
        duration_ms: opt_int,
        is_current: bool,
        confirmed_by: opt_text,
        batch_id: opt_id,
        created_by: text,
        created_at: ts,
    }

    ClassificationGoldLabel key(id: id) {
        id: id,
        workspace_id: id,
        task_id: opt_id,
        title: text,
        description: text,
        category: text,
        labeled_by: text,
        created_at: ts,
    }

    ClassificationEvaluation key(id: id) {
        id: id,
        workspace_id: id,
        provider: text,
        model: text,
        prompt_version: text,
        taxonomy_version: text,
        sample_size: int,
        failures: int,
        accuracy: opt_real,
        macro_f1: opt_real,
        per_class: object,
        confusion: object,
        results: objects,
        created_at: ts,
    }

    TaskEmbedding key(id: id) {
        id: id,
        task_id: id,
        provider: text,
        model: text,
        dimensions: int,
        vector: blob,
        input_hash: text,
        created_at: ts,
    }

    AutomationRule key(id: id) {
        id: id,
        workspace_id: id,
        name: text,
        description: text,
        enabled: bool,
        trigger: object,
        conditions: object,
        actions: objects,
        retry_max_attempts: int,
        max_depth: int,
        last_scheduled_at: opt_ts,
        next_run_at: opt_ts,
        created_by: text,
        version: int,
        created_at: ts,
        updated_at: ts,
    }

    AutomationRun key(id: id) {
        id: id,
        rule_id: id,
        workspace_id: id,
        trigger_type: text,
        trigger_key: text,
        trigger_event_id: opt_id,
        depth: int,
        dry_run: bool,
        status: text,
        matched: bool,
        reason: text,
        condition_trace: objects,
        action_results: objects,
        error: opt_text,
        attempts: int,
        correlation_id: opt_correlation,
        created_at: ts,
        finished_at: opt_ts,
        inputs: object,
    }

    Notification key(id: id) {
        id: id,
        workspace_id: id,
        level: text,
        title: text,
        body: text,
        source: text,
        data: object,
        read_at: opt_ts,
        created_at: ts,
    }

    Report key(id: id) {
        id: id,
        workspace_id: id,
        kind: text,
        title: text,
        scope: object,
        content_markdown: text,
        data: object,
        narrative_source: text,
        created_by: text,
        created_at: ts,
    }

    Finding key(id: id) {
        id: id,
        workspace_id: id,
        kind: text,
        fingerprint: text,
        title: text,
        severity: text,
        metric_value: opt_real,
        sample_size: int,
        evidence: object,
        first_detected_at: ts,
        last_detected_at: ts,
    }

    AgentSession key(id: id) {
        id: id,
        provider: text,
        external_session_id: text,
        workspace_id: opt_id,
        attempt_id: opt_id,
        task_id: opt_id,
        binding_source: text,
        cwd: opt_text,
        model: opt_text,
        transcript_path: opt_text,
        started_at: opt_ts,
        ended_at: opt_ts,
        end_reason: opt_text,
        tool_calls: int,
        tool_failures: int,
        created_at: ts,
        updated_at: ts,
    }

    SessionBinding key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        provider: text,
        external_session_id: opt_text,
        cwd_prefix: opt_text,
        active: bool,
        created_by: text,
        created_at: ts,
    }

    SubagentRun key(id: id) {
        id: id,
        session_id: id,
        external_agent_id: text,
        agent_type: opt_text,
        attempt_id: opt_id,
        started_at: opt_ts,
        stopped_at: opt_ts,
        last_message_excerpt: opt_text,
        transcript_path: opt_text,
        requested_model: opt_text,
        model: opt_text,
        usage: opt_object,
        telemetry: opt_object,
        task_id: opt_id,
    }

    AgentInternalTask key(id: id) {
        id: id,
        session_id: id,
        external_task_id: text,
        subject: text,
        description: opt_text,
        status: text,
        task_id: opt_id,
        external_agent_id: opt_text,
        created_at: ts,
        completed_at: opt_ts,
    }

    AgentProfile key(id: id) {
        id: id,
        workspace_id: id,
        name: text,
        adapter: opt_text,
        model: opt_text,
        permissions: strings,
        enabled: bool,
        last_seen_at: opt_ts,
        created_at: ts,
    }

    HookEvent key(id: id) {
        id: id,
        dedupe_key: text,
        provider: text,
        hook_event_name: text,
        external_session_id: opt_text,
        session_id: opt_id,
        attempt_id: opt_id,
        payload: opt_object,
        payload_purged_at: opt_ts,
        received_at: ts,
        source: text,
    }

    ToolEvent key(id: id) {
        id: id,
        hook_event_id: id,
        session_id: opt_id,
        attempt_id: opt_id,
        external_agent_id: opt_text,
        phase: text,
        tool_name: text,
        tool_use_id: opt_text,
        duration_ms: opt_int,
        error_excerpt: opt_text,
        summary: object,
        occurred_at: ts,
    }

    Experiment key(id: id) {
        id: id,
        workspace_id: id,
        slug: text,
        title: text,
        question: opt_text,
        status: text,
        protocol: object,
        frozen_at: opt_ts,
        created_at: ts,
        updated_at: ts,
    }

    ExperimentTrial key(id: id) {
        id: id,
        experiment_id: id,
        task_id: id,
        attempt_id: opt_id,
        subagent_run_id: opt_id,
        arm: text,
        requested_model: text,
        actual_model: opt_text,
        phase: text,
        seq: int,
        band: opt_int,
        features: object,
        base_ref: opt_text,
        worktree_path: opt_text,
        status: text,
        outcome: opt_text,
        outcome_detail: object,
        failure_cause: opt_text,
        failure_cause_source: opt_text,
        notes: opt_text,
        created_at: ts,
        started_at: opt_ts,
        graded_at: opt_ts,
    }

    PredictorFit key(id: id) {
        id: id,
        workspace_id: id,
        model: text,
        created_at: ts,
        data_cutoff: opt_ts,
        trial_ids: strings,
        feature_names: strings,
        repositories: strings,
        coef: values,
        penalty: values,
        n: int,
        converged: bool,
        metrics: object,
        samples: values,
    }

    AgentAttempt key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        number: int,
        status: parsed,
        provider: text,
        adapter: text,
        agent_id: opt_text,
        model: opt_text,
        session_id: opt_text,
        parent_attempt_id: opt_id,
        retry_of_attempt_id: opt_id,
        rerun_of_attempt_id: opt_id,
        created_by: text,
        created_at: ts,
        queued_at: opt_ts,
        started_at: opt_ts,
        finished_at: opt_ts,
        queue_wait_ms: opt_int,
        duration_ms: opt_int,
        exit_reason: opt_text,
        exit_code: opt_int,
        error: opt_text,
        summary: opt_text,
        input_task_version: opt_int,
        input_task_snapshot: object,
        prompt_artifact_id: opt_id,
        config_snapshot: object,
        environment: object,
        baseline_manifest_id: opt_id,
        outcome_manifest_id: opt_id,
        usage: object,
        usage_source: opt_text,
        cost_usd: opt_real,
        cost_source: opt_text,
        telemetry: object,
        modified_repository_ids: strings,
        retries: int,
        cancel_requested: bool,
        updated_at: ts,
    }

    RevisionManifest key(id: id) {
        id: id,
        workspace_id: id,
        attempt_id: opt_id,
        kind: text,
        entries: objects,
        created_at: ts,
    }

    ManagedWorktree key(id: id) {
        id: id,
        workspace_id: id,
        repository_id: id,
        source_checkout_id: id,
        attempt_id: opt_id,
        path: text,
        branch: text,
        base_sha: text,
        status: parsed,
        last_error: opt_text,
        created_at: ts,
        updated_at: ts,
        removed_at: opt_ts,
    }

    ResourceLease key(resource_key: text) {
        resource_key: text,
        holder: text,
        purpose: text,
        acquired_at: ts,
        expires_at: opt_ts,
    }

    ChangeSet key(id: id) {
        id: id,
        workspace_id: id,
        task_id: id,
        attempt_id: opt_id,
        title: text,
        status: parsed,
        created_at: ts,
        updated_at: ts,
    }

    ChangeSetItem key(id: id) {
        id: id,
        change_set_id: id,
        repository_id: id,
        base_sha: opt_text,
        head_sha: opt_text,
        branch: opt_text,
        commit_shas: strings,
        status: parsed,
        detail: opt_text,
        updated_at: ts,
    }

    Artifact key(id: id) {
        id: id,
        workspace_id: id,
        task_id: opt_id,
        attempt_id: opt_id,
        kind: text,
        name: text,
        media_type: text,
        path: text,
        size_bytes: int,
        sha256: text,
        retention: text,
        purged_at: opt_ts,
        metadata: object,
        created_at: ts,
    }

    Stack key(id: id) {
        id: id,
        workspace_id: id,
        parent_task_id: id,
        status: parsed,
        created_at: ts,
        updated_at: ts,
        land_request: opt_text,
    }

    StackRepository key{stack_id: id, repository_id: id} {
        stack_id: id,
        repository_id: id,
        base_sha: text,
        integration_branch: text,
        created_at: ts,
    }

    StackItem key(id: id) {
        id: id,
        stack_id: id,
        task_id: id,
        state: parsed,
        merge_order: opt_int,
        detail: opt_text,
        created_at: ts,
        updated_at: ts,
    }

    Job key(id: id) {
        id: id,
        workspace_id: opt_id,
        kind: text,
        status: parsed,
        priority: int,
        payload: object,
        result: opt_object,
        run_at: ts,
        attempts: int,
        max_attempts: int,
        backoff_base_seconds: real,
        timeout_seconds: opt_real,
        lease_owner: opt_text,
        lease_expires_at: opt_ts,
        heartbeat_at: opt_ts,
        started_at: opt_ts,
        finished_at: opt_ts,
        cancel_requested: bool,
        idempotency_key: opt_text,
        resource_keys: strings,
        last_error: opt_text,
        error_kind: opt_text,
        correlation_id: opt_correlation,
        causation_id: opt_id,
        task_id: opt_id,
        attempt_id: opt_id,
        created_by: text,
        created_at: ts,
        updated_at: ts,
    }

    JobRun key(id: id) {
        id: id,
        job_id: id,
        attempt_number: int,
        worker_id: text,
        status: text,
        error: opt_text,
        started_at: ts,
        finished_at: opt_ts,
    }

    JobResourceClaim key{job_id: id, resource_key: text} {
        job_id: id,
        resource_key: text,
    }

    Worker key(id: text) {
        id: text,
        hostname: text,
        pid: int,
        status: text,
        concurrency: int,
        kinds: strings,
        current_jobs: strings,
        jobs_completed: int,
        jobs_failed: int,
        started_at: ts,
        heartbeat_at: ts,
        stopped_at: opt_ts,
    }

    SystemCursor key(name: text) {
        name: text,
        value: int,
        updated_at: ts,
    }
}
