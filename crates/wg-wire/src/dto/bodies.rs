//! The request bodies (and the small result records) of the HTTP API, with their Pydantic field
//! orders.
//!
//! A body is input, so it has no `from_model`: the server decodes it with the request validation
//! of `wg-mcpspec` and the HTTP layer. What the wire crate fixes is the schema name and the field
//! order, which the component-schema test checks against the OpenAPI snapshot and the Python
//! source (`core/models/schemas.py` and `api/routes/*.py`). The analytics and report bodies keep
//! their own modules (`analytics_compare_body` and friends).

/// One request body (or result record): the OpenAPI component schema name and the field names in
/// Python declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body {
    /// The OpenAPI component schema this body mirrors.
    pub name: &'static str,
    /// The fields in Python declaration order.
    pub fields: &'static [&'static str],
}

const fn body(name: &'static str, fields: &'static [&'static str]) -> Body {
    Body { name, fields }
}

// Workspaces, projects and repositories.
pub const WORKSPACE_CREATE: Body = body(
    "WorkspaceCreate",
    &["slug", "name", "description", "key_prefix"],
);
pub const PROJECT_CREATE: Body = body(
    "ProjectCreate",
    &["slug", "name", "description", "repositories", "settings"],
);
pub const PROJECT_UPDATE: Body = body(
    "ProjectUpdate",
    &["slug", "name", "description", "status", "settings"],
);
pub const REPOSITORY_REGISTER: Body = body(
    "RepositoryRegister",
    &[
        "path",
        "name",
        "description",
        "access_mode",
        "tracked_branches",
        "projects",
    ],
);
pub const REPOSITORY_UPDATE: Body = body(
    "RepositoryUpdate",
    &[
        "name",
        "description",
        "access_mode",
        "default_branch",
        "tracked_branches",
    ],
);
pub const REPO_LINK: Body = body("RepoLink", &["repository", "role"]);

// Tasks.
pub const EFFORT_ESTIMATE: Body = body("EffortEstimate", &["value", "unit", "source"]);
pub const TASK_CREATE: Body = body(
    "TaskCreate",
    &[
        "key",
        "title",
        "description",
        "acceptance_criteria",
        "status",
        "priority",
        "tags",
        "files",
        "projects",
        "repositories",
        "parent",
        "depends_on",
        "assignee",
        "assigned_agent",
        "auto_execute",
        "due_at",
        "scheduled_at",
        "estimated_effort",
        "metadata",
    ],
);
pub const TASK_UPDATE: Body = body(
    "TaskUpdate",
    &[
        "expected_version",
        "title",
        "description",
        "acceptance_criteria",
        "status",
        "priority",
        "tags",
        "add_tags",
        "remove_tags",
        "files",
        "add_files",
        "projects",
        "parent",
        "clear_parent",
        "assignee",
        "assigned_agent",
        "auto_execute",
        "due_at",
        "scheduled_at",
        "estimated_effort",
        "metadata",
        "reason",
    ],
);
pub const TASK_FILE_IN: Body = body("TaskFileIn", &["path", "anchor", "note"]);
pub const BULK_UPDATE: Body = body("BulkUpdate", &["tasks", "update"]);
pub const BULK_RESULT: Body = body("BulkResult", &["updated", "failed"]);
pub const RELATION_CREATE: Body = body("RelationCreate", &["target", "relation_type"]);
pub const COMMENT_CREATE: Body = body("CommentCreate", &["body", "kind", "data", "attempt_id"]);
pub const DEPENDENCY_BODY: Body = body("DependencyBody", &["depends_on"]);
pub const LINK_COMMIT_BODY: Body = body("LinkCommitBody", &["sha", "repository", "note"]);
pub const REPO_BODY: Body = body("RepoBody", &["repository", "access"]);
pub const CLAIM_REQUEST: Body = body(
    "ClaimRequest",
    &["agent", "lease_minutes", "expected_version"],
);
pub const PROGRESS_REPORT: Body = body(
    "ProgressReport",
    &["message", "percent", "attempt_id", "data"],
);
pub const RESULT_REPORT: Body = body(
    "ResultReport",
    &[
        "attempt_id",
        "outcome",
        "summary",
        "artifacts",
        "usage",
        "cost_usd",
    ],
);
pub const REASON_BODY: Body = body("ReasonBody", &["reason"]);
pub const COMPLETE_BODY: Body = body("CompleteBody", &["summary", "attempt_id"]);
pub const ACCEPT_BODY: Body = body("AcceptBody", &["manual", "reason"]);
pub const REJECT_BODY: Body = body("RejectBody", &["reason"]);
pub const CONFIRM_BODY: Body = body("ConfirmBody", &["category", "note", "complexity"]);

// Runs, verification, automation and settings.
pub const RUN_START: Body = body("RunStart", &["task", "agent", "model", "instructions"]);
pub const CANCEL_BODY: Body = body("CancelBody", &["reason"]);
pub const VERIFICATION_CONFIG_BODY: Body = body(
    "VerificationConfigBody",
    &["scope_type", "scope", "name", "checks"],
);
pub const PREVIEW_BODY: Body = body("PreviewBody", &["rule", "spec", "event_id", "inputs"]);
pub const SETTINGS_PATCH: Body = body("SettingsPatch", &["values"]);

/// Every body and result record of this module.
pub const ALL: &[Body] = &[
    WORKSPACE_CREATE,
    PROJECT_CREATE,
    PROJECT_UPDATE,
    REPOSITORY_REGISTER,
    REPOSITORY_UPDATE,
    REPO_LINK,
    EFFORT_ESTIMATE,
    TASK_CREATE,
    TASK_UPDATE,
    TASK_FILE_IN,
    BULK_UPDATE,
    BULK_RESULT,
    RELATION_CREATE,
    COMMENT_CREATE,
    DEPENDENCY_BODY,
    LINK_COMMIT_BODY,
    REPO_BODY,
    CLAIM_REQUEST,
    PROGRESS_REPORT,
    RESULT_REPORT,
    REASON_BODY,
    COMPLETE_BODY,
    ACCEPT_BODY,
    REJECT_BODY,
    CONFIRM_BODY,
    RUN_START,
    CANCEL_BODY,
    VERIFICATION_CONFIG_BODY,
    PREVIEW_BODY,
    SETTINGS_PATCH,
];
