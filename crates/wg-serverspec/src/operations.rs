//! The 130 HTTP operations of the inventory (H001-H130), in the order the Python application
//! registers its routes, and the interface task that implements each group.
//!
//! The order matters: like Starlette, the router tries routes in registration order, so the
//! first template that matches a path wins (`/attempts/compare` before `/attempts/{attempt_id}`),
//! and a 405's `Allow` names the first route whose path matched.

/// One operation: its inventory id, method, path template and success status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operation {
    pub id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub status: u16,
}

const fn op(id: &'static str, method: &'static str, path: &'static str, status: u16) -> Operation {
    Operation {
        id,
        method,
        path,
        status,
    }
}

/// Every operation, in registration order (routers `core`, `tasks`, `decisions`, `stacks`, `ops`).
pub const OPERATIONS: [Operation; 130] = [
    op("H001", "GET", "/api/v1/health", 200),
    op("H002", "GET", "/api/v1/meta", 200),
    op("H003", "GET", "/api/v1/workspaces", 200),
    op("H004", "POST", "/api/v1/workspaces", 201),
    op("H005", "GET", "/api/v1/workspaces/{ws}", 200),
    op("H006", "GET", "/api/v1/ws/{ws}/projects", 200),
    op("H007", "POST", "/api/v1/ws/{ws}/projects", 201),
    op("H008", "GET", "/api/v1/ws/{ws}/projects/{ref}", 200),
    op("H009", "PATCH", "/api/v1/ws/{ws}/projects/{ref}", 200),
    op(
        "H010",
        "POST",
        "/api/v1/ws/{ws}/projects/{ref}/repositories",
        200,
    ),
    op(
        "H011",
        "DELETE",
        "/api/v1/ws/{ws}/projects/{ref}/repositories/{repository}",
        200,
    ),
    op("H012", "GET", "/api/v1/ws/{ws}/repositories", 200),
    op("H013", "POST", "/api/v1/ws/{ws}/repositories", 201),
    op("H014", "GET", "/api/v1/ws/{ws}/repositories/{ref}", 200),
    op("H015", "PATCH", "/api/v1/ws/{ws}/repositories/{ref}", 200),
    op("H016", "DELETE", "/api/v1/ws/{ws}/repositories/{ref}", 200),
    op(
        "H017",
        "POST",
        "/api/v1/ws/{ws}/repositories/{ref}/refresh",
        200,
    ),
    op(
        "H018",
        "GET",
        "/api/v1/ws/{ws}/repositories/{ref}/status",
        200,
    ),
    op(
        "H019",
        "POST",
        "/api/v1/ws/{ws}/repositories/{ref}/sync",
        200,
    ),
    op(
        "H020",
        "GET",
        "/api/v1/ws/{ws}/repositories/{ref}/commits",
        200,
    ),
    op(
        "H021",
        "GET",
        "/api/v1/ws/{ws}/repositories/{ref}/commits/{sha}",
        200,
    ),
    op("H022", "GET", "/api/v1/ws/{ws}/worktrees", 200),
    op("H023", "GET", "/api/v1/ws/{ws}/tasks", 200),
    op("H024", "POST", "/api/v1/ws/{ws}/tasks", 201),
    op("H025", "POST", "/api/v1/ws/{ws}/tasks/bulk", 200),
    op("H026", "GET", "/api/v1/ws/{ws}/ready", 200),
    op("H027", "GET", "/api/v1/ws/{ws}/graph", 200),
    op("H028", "GET", "/api/v1/ws/{ws}/tasks/{ref}", 200),
    op("H029", "PATCH", "/api/v1/ws/{ws}/tasks/{ref}", 200),
    op("H030", "POST", "/api/v1/ws/{ws}/tasks/{ref}/claim", 200),
    op("H031", "POST", "/api/v1/ws/{ws}/tasks/{ref}/release", 200),
    op("H032", "POST", "/api/v1/ws/{ws}/tasks/{ref}/complete", 200),
    op("H033", "POST", "/api/v1/ws/{ws}/tasks/{ref}/accept", 200),
    op("H034", "POST", "/api/v1/ws/{ws}/tasks/{ref}/reject", 200),
    op(
        "H035",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/acceptances",
        200,
    ),
    op("H036", "POST", "/api/v1/ws/{ws}/tasks/{ref}/comments", 201),
    op("H037", "POST", "/api/v1/ws/{ws}/tasks/{ref}/progress", 200),
    op("H038", "POST", "/api/v1/ws/{ws}/tasks/{ref}/result", 200),
    op(
        "H039",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/dependencies",
        200,
    ),
    op(
        "H040",
        "DELETE",
        "/api/v1/ws/{ws}/tasks/{ref}/dependencies/{depends_on}",
        200,
    ),
    op("H041", "POST", "/api/v1/ws/{ws}/tasks/{ref}/relations", 200),
    op(
        "H042",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/repositories",
        200,
    ),
    op(
        "H043",
        "DELETE",
        "/api/v1/ws/{ws}/tasks/{ref}/repositories/{repository}",
        200,
    ),
    op("H044", "POST", "/api/v1/ws/{ws}/tasks/{ref}/archive", 200),
    op("H045", "GET", "/api/v1/ws/{ws}/tasks/{ref}/history", 200),
    op("H046", "GET", "/api/v1/ws/{ws}/tasks/{ref}/attempts", 200),
    op("H047", "GET", "/api/v1/ws/{ws}/tasks/{ref}/commits", 200),
    op("H048", "POST", "/api/v1/ws/{ws}/tasks/{ref}/commits", 200),
    op("H049", "GET", "/api/v1/ws/{ws}/tasks/{ref}/changes", 200),
    op("H050", "GET", "/api/v1/ws/{ws}/tasks/{ref}/diff", 200),
    op(
        "H051",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/change-sets",
        200,
    ),
    op(
        "H052",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/classification",
        200,
    ),
    op(
        "H053",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/classification",
        200,
    ),
    op(
        "H054",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/classification/confirm",
        200,
    ),
    op("H055", "GET", "/api/v1/ws/{ws}/tasks/{ref}/similar", 200),
    op("H056", "GET", "/api/v1/ws/{ws}/tasks/{ref}/estimate", 200),
    op(
        "H057",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/verification",
        200,
    ),
    op("H058", "GET", "/api/v1/ws/{ws}/tasks/{ref}/checks", 200),
    op("H059", "POST", "/api/v1/ws/{ws}/tasks/{ref}/verify", 200),
    op("H060", "POST", "/api/v1/ws/{ws}/tasks/{ref}/decompose", 200),
    op(
        "H061",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/schedulability",
        200,
    ),
    op("H125", "GET", "/api/v1/ws/{ws}/decisions", 200),
    op(
        "H126",
        "GET",
        "/api/v1/ws/{ws}/decisions/{decision_id}",
        200,
    ),
    op("H129", "GET", "/api/v1/ws/{ws}/tasks/{ref}/decisions", 200),
    op("H130", "POST", "/api/v1/ws/{ws}/tasks/{ref}/decisions", 201),
    op(
        "H127",
        "POST",
        "/api/v1/ws/{ws}/decisions/{decision_id}/choose",
        200,
    ),
    op(
        "H128",
        "POST",
        "/api/v1/ws/{ws}/decisions/{decision_id}/withdraw",
        200,
    ),
    op("H062", "GET", "/api/v1/ws/{ws}/stacks", 200),
    op("H063", "POST", "/api/v1/ws/{ws}/stacks", 201),
    op("H064", "GET", "/api/v1/ws/{ws}/stacks/{ref}", 200),
    op("H065", "POST", "/api/v1/ws/{ws}/stacks/{ref}/items", 201),
    op(
        "H066",
        "DELETE",
        "/api/v1/ws/{ws}/stacks/{ref}/items/{task}",
        200,
    ),
    op("H067", "GET", "/api/v1/ws/{ws}/stacks/{ref}/history", 200),
    op("H068", "GET", "/api/v1/ws/{ws}/tasks/{ref}/stacks", 200),
    op(
        "H069",
        "POST",
        "/api/v1/ws/{ws}/stacks/{ref}/integrate",
        202,
    ),
    op("H070", "POST", "/api/v1/ws/{ws}/stacks/{ref}/verify", 202),
    op("H071", "GET", "/api/v1/ws/{ws}/attempts", 200),
    op("H072", "POST", "/api/v1/ws/{ws}/runs", 201),
    op("H073", "GET", "/api/v1/attempts/compare", 200),
    op("H074", "GET", "/api/v1/attempts/{attempt_id}", 200),
    op(
        "H075",
        "GET",
        "/api/v1/attempts/{attempt_id}/manifests",
        200,
    ),
    op("H076", "GET", "/api/v1/attempts/{attempt_id}/log", 200),
    op("H077", "POST", "/api/v1/attempts/{attempt_id}/cancel", 200),
    op("H078", "POST", "/api/v1/attempts/{attempt_id}/retry", 200),
    op("H079", "POST", "/api/v1/attempts/{attempt_id}/rerun", 200),
    op("H080", "GET", "/api/v1/jobs", 200),
    op("H081", "GET", "/api/v1/jobs/{job_id}", 200),
    op("H082", "POST", "/api/v1/jobs/{job_id}/cancel", 200),
    op("H083", "POST", "/api/v1/jobs/{job_id}/retry", 200),
    op("H084", "GET", "/api/v1/workers", 200),
    op("H085", "GET", "/api/v1/ws/{ws}/verification/configs", 200),
    op("H086", "POST", "/api/v1/ws/{ws}/verification/configs", 201),
    op(
        "H087",
        "POST",
        "/api/v1/verification/configs/{config_id}/{action}",
        200,
    ),
    op("H088", "GET", "/api/v1/ws/{ws}/verification/runs", 200),
    op("H089", "GET", "/api/v1/verification/runs/{run_id}", 200),
    op("H090", "GET", "/api/v1/ws/{ws}/verification/failures", 200),
    op("H091", "GET", "/api/v1/automation/catalog", 200),
    op("H092", "GET", "/api/v1/ws/{ws}/automation/rules", 200),
    op("H093", "POST", "/api/v1/ws/{ws}/automation/rules", 201),
    op("H094", "GET", "/api/v1/ws/{ws}/automation/rules/{ref}", 200),
    op("H095", "PUT", "/api/v1/ws/{ws}/automation/rules/{ref}", 200),
    op(
        "H096",
        "POST",
        "/api/v1/ws/{ws}/automation/rules/{ref}/{action}",
        200,
    ),
    op("H097", "POST", "/api/v1/ws/{ws}/automation/preview", 200),
    op("H098", "GET", "/api/v1/ws/{ws}/automation/runs", 200),
    op("H099", "GET", "/api/v1/ws/{ws}/notifications", 200),
    op(
        "H100",
        "POST",
        "/api/v1/notifications/{notification_id}/read",
        200,
    ),
    op("H101", "GET", "/api/v1/analytics/metrics", 200),
    op("H102", "POST", "/api/v1/ws/{ws}/analytics/query", 200),
    op("H103", "GET", "/api/v1/ws/{ws}/analytics/overview", 200),
    op("H104", "POST", "/api/v1/ws/{ws}/analytics/compare", 200),
    op("H105", "GET", "/api/v1/ws/{ws}/analytics/coupling", 200),
    op("H106", "GET", "/api/v1/ws/{ws}/analytics/failures", 200),
    op("H107", "GET", "/api/v1/ws/{ws}/analytics/agent-time", 200),
    op("H108", "GET", "/api/v1/ws/{ws}/analytics/bottlenecks", 200),
    op("H109", "GET", "/api/v1/ws/{ws}/analytics/history", 200),
    op("H110", "POST", "/api/v1/ws/{ws}/analytics/estimate", 200),
    op("H111", "POST", "/api/v1/ws/{ws}/analytics/export", 200),
    op("H112", "GET", "/api/v1/ws/{ws}/reports", 200),
    op("H113", "POST", "/api/v1/ws/{ws}/reports/retrospective", 201),
    op("H114", "GET", "/api/v1/ws/{ws}/search", 200),
    op(
        "H115",
        "GET",
        "/api/v1/ws/{ws}/classification/evaluations",
        200,
    ),
    op(
        "H116",
        "GET",
        "/api/v1/ws/{ws}/classification/disagreements",
        200,
    ),
    op("H117", "GET", "/api/v1/settings", 200),
    op("H118", "PATCH", "/api/v1/settings", 200),
    op("H119", "GET", "/api/v1/agents", 200),
    op("H120", "GET", "/api/v1/llm/health", 200),
    op("H121", "GET", "/api/v1/claude/status", 200),
    op("H122", "GET", "/api/v1/ws/{ws}/scheduler", 200),
    op("H123", "GET", "/api/v1/ws/{ws}/events", 200),
    op("H124", "GET", "/api/v1/ws/{ws}/events/stream", 200),
];

/// The number part of an operation id (`H033` is 33).
fn number(id: &str) -> u32 {
    id.trim_start_matches('H').parse().unwrap_or(0)
}

/// The interface group an operation belongs to: the plan's task id for the route group (section
/// 9.9), or `decisions` for H125-H130, which no group task lists yet.
pub fn group_of(id: &str) -> &'static str {
    match number(id) {
        1..=2 | 117..=118 => "meta-settings",
        3..=5 | 119..=121 => "IF-42",
        6..=22 => "IF-43",
        33..=35 => "IF-24",
        23..=46 => "IF-44",
        47..=61 => "IF-45",
        62..=70 => "IF-53",
        71..=84 | 122 => "IF-46",
        85..=100 => "IF-47",
        101..=116 => "IF-48",
        123 | 124 => "IF-49",
        125..=130 => "decisions",
        _ => "unknown",
    }
}

/// Every group name, in order.
pub const GROUPS: [&str; 12] = [
    "meta-settings",
    "IF-42",
    "IF-43",
    "IF-44",
    "IF-24",
    "IF-45",
    "IF-53",
    "IF-46",
    "IF-47",
    "IF-48",
    "IF-49",
    "decisions",
];

/// The operation with this method and path template.
pub fn find_operation(method: &str, path: &str) -> Option<&'static Operation> {
    OPERATIONS
        .iter()
        .find(|op| op.method == method && op.path == path)
}

/// The position of an operation in registration order.
pub fn registration_index(id: &str) -> Option<usize> {
    OPERATIONS.iter().position(|op| op.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_has_a_group_and_is_in_the_snapshot() {
        let doc = wg_wire::contract::openapi().unwrap();
        let mut count = 0;
        for op in OPERATIONS {
            assert!(GROUPS.contains(&group_of(op.id)), "{}", op.id);
            let found = doc["paths"][op.path][op.method.to_ascii_lowercase()].is_object();
            assert!(found, "{} {} {}", op.id, op.method, op.path);
            count += 1;
        }
        let in_snapshot: usize = doc["paths"]
            .as_object()
            .unwrap()
            .values()
            .map(|ops| ops.as_object().unwrap().len())
            .sum();
        assert_eq!(count, in_snapshot);
        assert_eq!(group_of("H034"), "IF-24");
        assert_eq!(group_of("H036"), "IF-44");
        assert_eq!(group_of("H001"), "meta-settings");
        assert_eq!(group_of("H118"), "meta-settings");
        assert_eq!(group_of("H003"), "IF-42");
        assert_eq!(group_of("H121"), "IF-42");
    }
}
