//! wg-model checks for the knowledge families: their event payloads against contracts/events.json,
//! their tables against schema 0010, and their slices as values.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    AgentSession, AutomationRule, Classification, Commit, Experiment, ExperimentTrial, Real,
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
use wg_model::event::EventPayload;
use wg_model::event::{
    AgentInternalTaskChanged, AgentSessionBound, AgentSessionEnded, AgentSessionStarted,
    AgentSubagentChanged, AutomationRuleCreated, AutomationRuleToggled, AutomationRuleUpdated,
    AutomationRunCompleted, ClassificationCompleted, ClassificationConfirmed,
    ClassificationDisagreement, ClassificationEvaluated, ClassificationFailed,
    ClassificationPromoted, ClassificationSuperseded, ExperimentClosed, ExperimentCreated,
    ExperimentFrozen, ExperimentProtocolUpdated, ExperimentTrialAdded, ExperimentTrialClassified,
    ExperimentTrialGraded, ExperimentTrialInvalidated, ExperimentTrialLinked,
    ExperimentTrialStarted, GitCommitIngested, GitCommitsAttributed, GitSyncCompleted,
    NotificationCreated, PredictorFitted, ReportGenerated, VerificationCompleted,
    VerificationConfigToggled, VerificationConfigWritten, VerificationStarted,
};
use wg_model::slice::{IntelSlice, VerifySlice};
use wg_model::{Row, Table, Write, derive_id};
use wg_types::{Id, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

/// Event families this file covers, by name prefix.
const FAMILIES: &[&str] = &[
    "agent_session.",
    "automation.",
    "classification.",
    "experiment.",
    "experiment_trial.",
    "git.",
    "notification.",
    "predictor.",
    "report.",
    "verification.",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(3), n)
}

fn at() -> Timestamp {
    match Timestamp::from_components(2026, 10, 25, 12, 0, 0, 0) {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn contract_events() -> Result<serde_json::Map<String, Value>, Box<dyn Error>> {
    let text = std::fs::read_to_string(repo_root().join("contracts/events.json"))?;
    let parsed: Value = serde_json::from_str(&text)?;
    Ok(parsed
        .get("events")
        .and_then(Value::as_object)
        .ok_or("contracts/events.json has no events object")?
        .clone())
}

fn strings(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// One payload of every knowledge event type, with the values that the shape needs.
fn samples() -> Vec<EventPayload> {
    let task_id = id(10);
    let run_id = id(11);
    let mut out = vec![
        EventPayload::AgentSessionBound(AgentSessionBound {
            cwd: Some("C:/src/app".into()),
            session_id: Some("s-1".into()),
        }),
        EventPayload::AgentSessionStarted(AgentSessionStarted {
            binding: json!("task"),
            external_session_id: "s-1".into(),
            provider: "claude_code".into(),
            source: Some("hook".into()),
            task_id: Some(task_id),
        }),
        EventPayload::AgentSessionEnded(AgentSessionEnded {
            reason: None,
            tool_calls: 3,
            tool_failures: 0,
        }),
        EventPayload::AgentSessionInternalTaskCreated(AgentInternalTaskChanged {
            authoritative: false,
            external_task_id: "t-1".into(),
            mapped_task_id: None,
            subject: "Look".into(),
        }),
        EventPayload::AgentSessionInternalTaskCompleted(AgentInternalTaskChanged {
            authoritative: true,
            external_task_id: "t-1".into(),
            mapped_task_id: Some(task_id),
            subject: "Look".into(),
        }),
        EventPayload::AgentSessionSubagentStarted(AgentSubagentChanged {
            agent_id: "a-1".into(),
            agent_type: Some("Explore".into()),
            child_attempt_id: None,
        }),
        EventPayload::AgentSessionSubagentStopped(AgentSubagentChanged {
            agent_id: "a-1".into(),
            agent_type: None,
            child_attempt_id: Some(id(12)),
        }),
        EventPayload::AutomationRuleCreated(AutomationRuleCreated {
            name: "notify".into(),
            trigger: json!({"event": "task.completed"}),
        }),
        EventPayload::AutomationRuleUpdated(AutomationRuleUpdated { version: 2 }),
        EventPayload::AutomationRuleEnabled(AutomationRuleToggled {}),
        EventPayload::AutomationRuleDisabled(AutomationRuleToggled {}),
        EventPayload::AutomationRunCompleted(AutomationRunCompleted {
            actions: json!([]),
            error: None,
            rule: "notify".into(),
            status: "succeeded".into(),
        }),
        EventPayload::NotificationCreated(NotificationCreated {
            level: "info".into(),
            title: "Done".into(),
        }),
        EventPayload::ReportGenerated(ReportGenerated {
            kind: "retrospective".into(),
            narrative_source: "deterministic".into(),
        }),
        EventPayload::ClassificationCompleted(ClassificationCompleted {
            category: Some("feature".into()),
            classification_id: id(13),
            confidence: Some(Real(0.75)),
            is_current: true,
            model: Some("m".into()),
            prompt_version: Some("p1".into()),
            provider: Some("heuristic".into()),
            taxonomy_version: "t1".into(),
        }),
        EventPayload::ClassificationConfirmed(ClassificationConfirmed {
            category: Some("feature".into()),
            classification_id: id(13),
            previous: None,
        }),
        EventPayload::ClassificationDisagreement(ClassificationDisagreement {
            current: json!("bug"),
            new: json!("feature"),
            replaced: json!(true),
        }),
        EventPayload::ClassificationEvaluated(ClassificationEvaluated {
            accuracy: None,
            evaluation_id: id(14),
            model: "m".into(),
            provider: "heuristic".into(),
            sample_size: 5,
        }),
        EventPayload::ClassificationFailed(ClassificationFailed {
            classification_id: id(13),
            error: Some("timeout".into()),
            model: None,
            provider: None,
            reason: "provider".into(),
        }),
        EventPayload::ClassificationPromoted(ClassificationPromoted {
            classification_id: id(13),
            previous_id: Some(id(15)),
        }),
        EventPayload::ClassificationSuperseded(ClassificationSuperseded {
            new_id: id(13),
            previous_id: None,
            reason: "rerun".into(),
        }),
        EventPayload::ExperimentCreated(ExperimentCreated {
            arms: json!(["a", "b"]),
            slug: "exp".into(),
        }),
        EventPayload::ExperimentFrozen(ExperimentFrozen {
            protocol: serde_json::Map::new(),
            slug: "exp".into(),
        }),
        EventPayload::ExperimentProtocolUpdated(ExperimentProtocolUpdated {
            protocol: serde_json::Map::new(),
            slug: "exp".into(),
        }),
        EventPayload::ExperimentClosed(ExperimentClosed {
            slug: "exp".into(),
            trials: json!(4),
        }),
        EventPayload::ExperimentTrialAdded(ExperimentTrialAdded {
            arm: "a".into(),
            band: Some(2),
            experiment: "exp".into(),
            phase: "main".into(),
            requested_model: "m".into(),
            seq: 1,
            task_key: "WG-1".into(),
        }),
        EventPayload::ExperimentTrialStarted(ExperimentTrialStarted {
            base_ref: None,
            experiment: "exp".into(),
            worktree_path: Some("C:/wt".into()),
        }),
        EventPayload::ExperimentTrialLinked(ExperimentTrialLinked {
            actual_model: None,
            attempt_id: None,
            experiment: "exp".into(),
            requested_model: "m".into(),
            subagent_run_id: Some(id(16)),
        }),
        EventPayload::ExperimentTrialGraded(ExperimentTrialGraded {
            experiment: "exp".into(),
            outcome: Some("pass".into()),
            status: "graded".into(),
        }),
        EventPayload::ExperimentTrialClassified(ExperimentTrialClassified {
            experiment: "exp".into(),
            failure_cause: None,
            failure_cause_source: None,
        }),
        EventPayload::ExperimentTrialInvalidated(ExperimentTrialInvalidated {
            actual_model: None,
            experiment: "exp".into(),
            failure_cause: Some("infra".into()),
            failure_cause_source: Some("manual".into()),
            requested_model: None,
        }),
        EventPayload::PredictorFitted(PredictorFitted {
            converged: true,
            data_cutoff: None,
            model: "ridge".into(),
            n: 40,
            repositories: json!(["app"]),
            samples: json!([]),
        }),
        EventPayload::GitCommitIngested(GitCommitIngested {
            deletions: 1,
            insertions: 2,
            repository_id: id(17),
            sha: "abc".into(),
            subject: "Fix".into(),
            task_ids: VectorSync::new_sync().push_back(task_id),
        }),
        EventPayload::GitCommitsAttributed(GitCommitsAttributed {
            links_created: 1,
            repository_id: None,
        }),
        EventPayload::GitSyncCompleted(GitSyncCompleted {
            equivalences: json!(0),
            head_refs: json!({}),
            links_created: 0,
            marked_unreachable: 0,
            mode: "incremental".into(),
            new_commits: 3,
            refs_changed: json!([]),
            repository: "app".into(),
            repository_id: id(17),
            restored_reachable: 0,
            shallow: false,
        }),
        EventPayload::VerificationConfigCreated(VerificationConfigWritten {
            checks: json!([]),
            name: "default".into(),
            scope_id: id(18),
            scope_type: "workspace".into(),
        }),
        EventPayload::VerificationConfigUpdated(VerificationConfigWritten {
            checks: json!([]),
            name: "default".into(),
            scope_id: id(18),
            scope_type: "workspace".into(),
        }),
        EventPayload::VerificationConfigEnabled(VerificationConfigToggled {}),
        EventPayload::VerificationConfigDisabled(VerificationConfigToggled {}),
        EventPayload::VerificationStarted(VerificationStarted {
            checks: json!([]),
            manifest_id: None,
            task_id,
            task_key: "WG-1".into(),
            tier: Some("task".into()),
            stack: None,
        }),
        EventPayload::VerificationCompleted(VerificationCompleted {
            failed_checks: json!([]),
            optional_failures: json!([]),
            required_passed: true,
            status: "passed".into(),
            task_id,
            task_key: "WG-1".into(),
            stack: Some(json!({"id": run_id.to_string()})),
        }),
    ];
    out.sort_by_key(|payload| payload.event_type());
    out
}

#[test]
fn knowledge_event_types_are_the_contract_types_of_their_families() -> TestResult {
    let contract = contract_events()?;
    let expected: BTreeSet<&str> = contract
        .keys()
        .map(String::as_str)
        .filter(|name| FAMILIES.iter().any(|family| name.starts_with(family)))
        .collect();
    let sampled: BTreeSet<&str> = samples().iter().map(EventPayload::event_type).collect();
    assert_eq!(sampled, expected, "every knowledge event type has a sample");
    Ok(())
}

#[test]
fn knowledge_payload_keys_and_entity_types_match_the_contract() -> TestResult {
    let contract = contract_events()?;
    for payload in samples() {
        let name = payload.event_type();
        let spec = contract
            .get(name)
            .ok_or(format!("{name} not in contract"))?;
        let required = strings(spec.get("data_keys"));
        let optional = strings(spec.get("optional_data_keys"));
        let entities = strings(spec.get("entity_types"));
        assert!(
            entities.contains(payload.entity_type().as_str()),
            "{name}: entity type {}",
            payload.entity_type().as_str()
        );
        let data = payload.to_data()?;
        let keys: BTreeSet<String> = data.keys().cloned().collect();
        assert!(keys.is_superset(&required), "{name}: missing keys {keys:?}");
        let allowed: BTreeSet<String> = required.union(&optional).cloned().collect();
        assert!(keys.is_subset(&allowed), "{name}: unexpected keys {keys:?}");
        assert_eq!(payload.schema_version(), 1, "{name}: schema version");
    }
    Ok(())
}

#[test]
fn absent_optional_keys_are_left_out_of_the_data() -> TestResult {
    let stack_less = EventPayload::VerificationStarted(VerificationStarted {
        checks: json!([]),
        manifest_id: None,
        task_id: id(1),
        task_key: "WG-1".into(),
        tier: Some("task".into()),
        stack: None,
    });
    assert!(!stack_less.to_data()?.contains_key("stack"));

    let invalidated = EventPayload::ExperimentTrialInvalidated(ExperimentTrialInvalidated {
        actual_model: None,
        experiment: "exp".into(),
        failure_cause: None,
        failure_cause_source: None,
        requested_model: Some("m".into()),
    });
    let data = invalidated.to_data()?;
    assert!(!data.contains_key("actual_model"));
    assert_eq!(data.get("requested_model"), Some(&json!("m")));
    assert_eq!(data.get("failure_cause"), Some(&Value::Null));
    Ok(())
}

#[test]
fn real_values_compare_by_bits() {
    assert_eq!(Real(0.5), Real(0.5));
    assert_ne!(Real(0.5), Real(0.25));
    assert_eq!(Real(f64::NAN), Real(f64::NAN));
    assert_ne!(Real(0.0), Real(-0.0));
}

#[test]
fn knowledge_tables_are_schema_tables_with_their_names() -> TestResult {
    let schema = std::fs::read_to_string(repo_root().join("contracts/schema-0010.sql"))?;
    let knowledge = [
        Table::Commit,
        Table::CommitFileChange,
        Table::CommitSymbol,
        Table::CommitAnalysis,
        Table::CommitEquivalence,
        Table::CommitTaskLink,
        Table::GitObjectGap,
        Table::TaskChangeSurface,
        Table::VerificationConfig,
        Table::VerificationRun,
        Table::VerificationCheckResult,
        Table::TaskAcceptance,
        Table::Classification,
        Table::ClassificationGoldLabel,
        Table::ClassificationEvaluation,
        Table::TaskEmbedding,
        Table::AutomationRule,
        Table::AutomationRun,
        Table::Notification,
        Table::Report,
        Table::Finding,
        Table::AgentSession,
        Table::SessionBinding,
        Table::SubagentRun,
        Table::AgentInternalTask,
        Table::AgentProfile,
        Table::HookEvent,
        Table::ToolEvent,
        Table::Experiment,
        Table::ExperimentTrial,
        Table::PredictorFit,
    ];
    // 18 base tables, 15 execution tables (`execution.rs`), then the knowledge tables.
    // 18 base tables plus task_decisions, 15 execution tables, then the knowledge tables.
    assert_eq!(Table::ALL.len(), 19 + 15 + knowledge.len());
    for table in knowledge {
        let name = table.name();
        let plain = format!("CREATE TABLE {name} (");
        let quoted = format!("CREATE TABLE \"{name}\" (");
        assert!(
            schema.contains(&plain) || schema.contains(&quoted),
            "{name} is not a table of schema 0010"
        );
    }
    Ok(())
}

#[test]
fn a_write_to_another_family_leaves_the_slice_unchanged() {
    let intel = IntelSlice::default();
    let commit = Commit {
        id: id(1),
        repository_id: id(2),
        sha: "abc".into(),
        tree_sha: None,
        parents: VectorSync::new_sync(),
        author_name: "a".into(),
        author_email: "a@example.test".into(),
        authored_at: None,
        committer_name: "a".into(),
        committer_email: "a@example.test".into(),
        committed_at: None,
        subject: "s".into(),
        message: "s".into(),
        trailers: VectorSync::new_sync(),
        is_merge: false,
        patch_id: None,
        insertions: 0,
        deletions: 0,
        files_changed: 0,
        reachable: true,
        unreachable_since: None,
        object_available: true,
        structure_analyzed: false,
        branches: VectorSync::new_sync(),
        ingested_at: at(),
    };
    let after = intel.apply(&Write::insert(commit));
    assert_eq!(after, intel);
}

#[test]
fn verification_rows_fold_into_and_out_of_the_slice() {
    let run = VerificationRun {
        id: id(1),
        workspace_id: id(2),
        task_id: id(3),
        attempt_id: None,
        manifest_id: None,
        config_snapshot: VectorSync::new_sync(),
        status: "pending".into(),
        required_passed: false,
        triggered_by: "system".into(),
        started_at: None,
        finished_at: None,
        created_at: at(),
    };
    let check = VerificationCheckResult {
        id: id(4),
        run_id: run.id,
        name: "tests".into(),
        kind: "command".into(),
        required: true,
        repository_id: None,
        status: "passed".into(),
        exit_code: Some(0),
        duration_ms: Some(12),
        timed_out: false,
        log_artifact_id: None,
        output_excerpt: String::new(),
        failure_signatures: VectorSync::new_sync(),
        created_at: at(),
    };
    let config = VerificationConfig {
        id: id(5),
        workspace_id: id(2),
        scope_type: "workspace".into(),
        scope_id: id(2),
        name: "default".into(),
        checks: VectorSync::new_sync(),
        enabled: true,
        approved_by: "human:Brice".into(),
        version: 1,
        created_at: at(),
        updated_at: at(),
    };
    let acceptance = TaskAcceptance {
        id: id(6),
        task_id: id(3),
        attempt_id: None,
        verification_run_id: Some(run.id),
        decision: "accepted".into(),
        mode: "automated".into(),
        reason: "checks passed".into(),
        actor: "system".into(),
        manifest_id: None,
        created_at: at(),
    };
    let writes = [
        Write::insert(config.clone()),
        Write::insert(run.clone()),
        Write::insert(check.clone()),
        Write::insert(acceptance.clone()),
    ];
    let filled = writes
        .iter()
        .fold(VerifySlice::default(), |slice, write| slice.apply(write));
    assert_eq!(filled.config(config.id), Some(&config));
    assert_eq!(filled.run(run.id), Some(&run));
    assert_eq!(filled.check_results_of(run.id).count(), 1);
    assert_eq!(filled.acceptances_of(run.task_id).count(), 1);

    let emptied = [
        Write::delete(config.row_key()),
        Write::delete(run.row_key()),
        Write::delete(check.row_key()),
        Write::delete(acceptance.row_key()),
    ]
    .iter()
    .fold(filled, |slice, write| slice.apply(write));
    assert_eq!(emptied, VerifySlice::default());
}

#[test]
fn classification_slice_finds_the_current_row() {
    let make = |n: u32, current: bool| Classification {
        id: id(n),
        task_id: id(100),
        source: "model".into(),
        status: "done".into(),
        provider: None,
        model: None,
        prompt_version: None,
        taxonomy_version: "t1".into(),
        input_hash: "h".into(),
        category: Some("feature".into()),
        subtype: None,
        domain: None,
        affected_systems: VectorSync::new_sync(),
        complexity: None,
        tags: VectorSync::new_sync(),
        confidence: Some(Real(0.9)),
        rationale: None,
        raw_output: None,
        validation_status: "valid".into(),
        error: None,
        duration_ms: None,
        is_current: current,
        confirmed_by: None,
        batch_id: None,
        created_by: "system".into(),
        created_at: at(),
    };
    let slice = [make(1, false), make(2, true)]
        .into_iter()
        .fold(IntelSlice::default(), |slice, row| {
            slice.apply(&Write::insert(row))
        });
    assert_eq!(slice.classifications_of(id(100)).count(), 2);
    assert_eq!(
        slice.current_classification(id(100)).map(|row| row.id),
        Some(id(2))
    );
}

#[test]
fn agent_session_and_experiment_rows_have_row_keys_in_their_table() {
    let session = AgentSession {
        id: id(1),
        provider: "claude_code".into(),
        external_session_id: "s-1".into(),
        workspace_id: None,
        attempt_id: None,
        task_id: None,
        binding_source: "unbound".into(),
        cwd: None,
        model: None,
        transcript_path: None,
        started_at: None,
        ended_at: None,
        end_reason: None,
        tool_calls: 0,
        tool_failures: 0,
        created_at: at(),
        updated_at: at(),
    };
    assert_eq!(Row::from(session.clone()).table(), Table::AgentSession);
    assert_eq!(session.row_key().table(), Table::AgentSession);

    let experiment = Experiment {
        id: id(2),
        workspace_id: id(3),
        slug: "exp".into(),
        title: "Exp".into(),
        question: None,
        status: "draft".into(),
        protocol: serde_json::Map::new(),
        frozen_at: None,
        created_at: at(),
        updated_at: at(),
    };
    assert_eq!(Row::from(experiment).table(), Table::Experiment);

    let trial = ExperimentTrial {
        id: id(4),
        experiment_id: id(2),
        task_id: id(5),
        attempt_id: None,
        subagent_run_id: None,
        arm: "a".into(),
        requested_model: "m".into(),
        actual_model: None,
        phase: "main".into(),
        seq: 1,
        band: None,
        features: serde_json::Map::new(),
        base_ref: None,
        worktree_path: None,
        status: "planned".into(),
        outcome: None,
        outcome_detail: serde_json::Map::new(),
        failure_cause: None,
        failure_cause_source: None,
        notes: None,
        created_at: at(),
        started_at: None,
        graded_at: None,
    };
    assert_eq!(Row::from(trial).table(), Table::ExperimentTrial);

    let rule = AutomationRule {
        id: id(6),
        workspace_id: id(3),
        name: "notify".into(),
        description: String::new(),
        enabled: true,
        trigger: serde_json::Map::new(),
        conditions: serde_json::Map::new(),
        actions: VectorSync::new_sync(),
        retry_max_attempts: 3,
        max_depth: 3,
        last_scheduled_at: None,
        next_run_at: None,
        created_by: "system".into(),
        version: 1,
        created_at: at(),
        updated_at: at(),
    };
    assert_eq!(Row::from(rule).table(), Table::AutomationRule);
}

#[test]
fn knowledge_event_count_matches_the_contract() -> TestResult {
    let contract = contract_events()?;
    let in_contract = contract
        .keys()
        .filter(|name| FAMILIES.iter().any(|family| name.starts_with(family)))
        .count();
    let implemented = EventPayload::EVENT_TYPES
        .iter()
        .filter(|name| FAMILIES.iter().any(|family| name.starts_with(family)))
        .count();
    assert_eq!(implemented, in_contract);
    assert_eq!(implemented, 41);
    Ok(())
}
