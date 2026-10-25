//! Permissions and the default sets per actor kind (`security/permissions.py`).

use std::collections::BTreeSet;

use crate::{ErrorCode, WgError};

string_enum! {
    pub enum Permission {
        WorkspaceRead = "workspace:read",
        WorkspaceAdmin = "workspace:admin",
        ProjectWrite = "project:write",
        RepoRead = "repo:read",
        RepoRegister = "repo:register",
        RepoAdmin = "repo:admin",
        TaskRead = "task:read",
        TaskWrite = "task:write",
        TaskClaim = "task:claim",
        TaskComplete = "task:complete",
        TaskAccept = "task:accept",
        TaskAdmin = "task:admin",
        GitRead = "git:read",
        GitIngest = "git:ingest",
        GitLink = "git:link",
        AttemptWrite = "attempt:write",
        StackWrite = "stack:write",
        ExperimentWrite = "experiment:write",
        RunStart = "run:start",
        RunCancel = "run:cancel",
        VerifyRun = "verify:run",
        AutomationRead = "automation:read",
        AutomationTrigger = "automation:trigger",
        AutomationManage = "automation:manage",
        ConfigManage = "config:manage",
        AnalyticsRead = "analytics:read",
        ClassifyRequest = "classification:request",
        ClassifyConfirm = "classification:confirm",
        JobManage = "job:manage",
        TelemetryIngest = "telemetry:ingest",
    }
}

/// Read-only permissions, granted to every actor kind.
pub fn read_permissions() -> BTreeSet<Permission> {
    [
        Permission::WorkspaceRead,
        Permission::RepoRead,
        Permission::TaskRead,
        Permission::GitRead,
        Permission::AutomationRead,
        Permission::AnalyticsRead,
    ]
    .into_iter()
    .collect()
}

/// Agents may track and report work, but not start execution, accept their own work, change
/// trusted configuration or author automation policy.
pub fn default_agent_permissions() -> BTreeSet<Permission> {
    let mut set = read_permissions();
    set.extend([
        Permission::TaskWrite,
        Permission::TaskClaim,
        Permission::TaskComplete,
        Permission::AttemptWrite,
        Permission::GitLink,
        Permission::ClassifyRequest,
        Permission::AutomationTrigger,
        Permission::TelemetryIngest,
    ]);
    set
}

/// Humans hold every permission.
pub fn human_permissions() -> BTreeSet<Permission> {
    all_permissions()
}

/// The system actor holds every permission.
pub fn system_permissions() -> BTreeSet<Permission> {
    all_permissions()
}

/// Every permission.
pub fn all_permissions() -> BTreeSet<Permission> {
    Permission::ALL.iter().copied().collect()
}

/// Parses permission strings; an unknown one is `permission_denied`, as in Python.
pub fn parse_permissions<S: AsRef<str>>(values: &[S]) -> Result<BTreeSet<Permission>, WgError> {
    values
        .iter()
        .map(|value| {
            Permission::parse(value.as_ref()).ok_or_else(|| {
                WgError::new(
                    ErrorCode::PermissionDenied,
                    format!("unknown permission {:?}", value.as_ref()),
                )
            })
        })
        .collect()
}
