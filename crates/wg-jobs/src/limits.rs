//! Resource keys and their concurrency limits (`jobs/limits.py`).
//!
//! A job names the resources it holds while running. A key's limit bounds how many running jobs
//! may hold it at once; a key with no limit is never held back.

use std::collections::BTreeSet;

pub const GPU: &str = "gpu";
pub const RUN: &str = "run";

pub fn agent_key(name: &str) -> String {
    format!("agent:{name}")
}

pub fn repo_key(repository_id: &str) -> String {
    format!("repo:{repository_id}")
}

/// Bounds the heavy verification jobs running on one repository (`jobs.verify_concurrency`).
pub fn verify_key(repository_id: &str) -> String {
    format!("verify:{repository_id}")
}

/// Held by the sync of one tracker connection, so at most one sync per connection runs at a time.
pub fn tracker_key(connection: &str) -> String {
    format!("tracker:{connection}")
}

/// Held exclusively by a stack's own integration and verification in one of its repositories.
pub fn stack_key(stack_id: &str, repository_id: &str) -> String {
    format!("exclusive:stack:{stack_id}:{repository_id}")
}

/// `verify:` keys for every repository a task is linked to, sorted (`task_verify_keys`).
pub fn task_verify_keys<'a>(repository_ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    repository_ids
        .into_iter()
        .map(verify_key)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The configured concurrency values the resolver reads (`config.jobs`, `config.execution`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitSettings {
    pub gpu_concurrency: i64,
    pub per_agent_concurrency: i64,
    pub per_repository_concurrency: i64,
    pub verify_concurrency: i64,
    pub max_concurrent_runs: i64,
}

impl LimitSettings {
    /// The limit of a resource key, `None` when the key is unlimited (`resource_limit_resolver`).
    pub fn limit_for(&self, key: &str) -> Option<i64> {
        if key == GPU {
            Some(self.gpu_concurrency)
        } else if key == RUN {
            Some(self.max_concurrent_runs)
        } else if key.starts_with("agent:") {
            Some(self.per_agent_concurrency)
        } else if key.starts_with("repo:") {
            Some(self.per_repository_concurrency)
        } else if key.starts_with("verify:") {
            Some(self.verify_concurrency)
        } else if key.starts_with("exclusive:") || key.starts_with("tracker:") {
            Some(1)
        } else {
            None
        }
    }
}
