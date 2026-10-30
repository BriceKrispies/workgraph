//! [`RequestContext`]: the acting identity and causal metadata of one request
//! (`core/context.py`), and the [`Stamp`] each decision of the request is given.
//!
//! A context lives for a whole request (one CLI command, one HTTP call, one job): every decision
//! made for it shares its correlation id. The shell draws that id and, per decision, the clock
//! reading and the entropy; [`RequestContext::stamp`] puts them together.

use std::collections::BTreeSet;

use rpds::RedBlackTreeSetSync;
use wg_model::{Stamp, py_repr_str};
use wg_types::{
    Actor, ActorKind, Correlation, Id, Permission, Timestamp, WgError, default_agent_permissions,
    human_permissions, system_permissions,
};

/// The environment variables `getpass.getuser()` reads, in its order.
pub const GETUSER_VARIABLES: [&str; 4] = ["LOGNAME", "USER", "LNAME", "USERNAME"];

/// `RequestContext`: who acts, with which permissions, and the causal chain it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContext {
    pub actor: Actor,
    pub permissions: RedBlackTreeSetSync<Permission>,
    /// `correlation_id`: shared by every event of the request.
    pub correlation: Correlation,
    /// `causation_id`: the event that caused this request.
    pub causation: Option<Id>,
    /// `attempt_id`: the attempt the request runs under.
    pub attempt: Option<Id>,
    /// `automation_depth`: 0 outside automation runs.
    pub automation_depth: u32,
}

fn permission_set(permissions: BTreeSet<Permission>) -> RedBlackTreeSetSync<Permission> {
    permissions.into_iter().collect()
}

impl RequestContext {
    /// A context with no causation, attempt or automation depth.
    pub fn new(
        kind: ActorKind,
        name: &str,
        permissions: BTreeSet<Permission>,
        correlation: impl Into<Correlation>,
    ) -> Result<Self, WgError> {
        Ok(Self {
            actor: Actor::new(kind, name)?,
            permissions: permission_set(permissions),
            correlation: correlation.into(),
            causation: None,
            attempt: None,
            automation_depth: 0,
        })
    }

    /// `RequestContext.human(name)`: `human:<user>` with every permission. Without a name, the
    /// login name found by [`getuser`] is used, and `local` when there is none.
    pub fn human(
        name: Option<&str>,
        login: Option<&str>,
        correlation: impl Into<Correlation>,
    ) -> Result<Self, WgError> {
        let user = name.filter(|n| !n.is_empty()).or(login).unwrap_or("local");
        Self::new(ActorKind::Human, user, human_permissions(), correlation)
    }

    /// `RequestContext.agent(name, permissions)`: the default agent permissions unless given.
    pub fn agent(
        name: &str,
        permissions: Option<BTreeSet<Permission>>,
        correlation: impl Into<Correlation>,
    ) -> Result<Self, WgError> {
        Self::new(
            ActorKind::Agent,
            name,
            permissions.unwrap_or_else(default_agent_permissions),
            correlation,
        )
    }

    /// `RequestContext.system(component)`: `system:<component>` with every permission.
    pub fn system(component: &str, correlation: impl Into<Correlation>) -> Result<Self, WgError> {
        Self::new(
            ActorKind::System,
            component,
            system_permissions(),
            correlation,
        )
    }

    /// `RequestContext.automation(rule_name, permissions, depth)`.
    pub fn automation(
        rule_name: &str,
        permissions: BTreeSet<Permission>,
        depth: u32,
        correlation: impl Into<Correlation>,
    ) -> Result<Self, WgError> {
        Ok(Self {
            automation_depth: depth,
            ..Self::new(ActorKind::Automation, rule_name, permissions, correlation)?
        })
    }

    /// `ctx.caused_by(event_id)`.
    #[must_use]
    pub fn caused_by(self, event: Option<Id>) -> Self {
        Self {
            causation: event,
            ..self
        }
    }

    /// `ctx.with_attempt(attempt_id)`.
    #[must_use]
    pub fn with_attempt(self, attempt: Option<Id>) -> Self {
        Self { attempt, ..self }
    }

    /// `ctx.deeper(causation_id)`: one automation level down, caused by `event`.
    #[must_use]
    pub fn deeper(self, event: Option<Id>) -> Self {
        Self {
            causation: event,
            automation_depth: self.automation_depth.saturating_add(1),
            ..self
        }
    }

    /// `security.permissions.require` for this request: fails with `permission_denied` naming
    /// every missing permission, sorted by wire string, as [`Stamp::require`] does.
    pub fn require(&self, permissions: &[Permission]) -> Result<(), WgError> {
        let missing: BTreeSet<&'static str> = permissions
            .iter()
            .filter(|permission| !self.permissions.contains(*permission))
            .map(|permission| permission.as_str())
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = missing.iter().copied().collect();
        let actor = self.actor.to_string();
        Err(WgError::permission_denied(format!(
            "actor {} lacks permission(s): {}",
            py_repr_str(&actor),
            names.join(", ")
        ))
        .detail("actor", serde_json::Value::String(actor))
        .detail(
            "missing",
            serde_json::Value::Array(
                names
                    .iter()
                    .map(|name| serde_json::Value::String((*name).to_owned()))
                    .collect(),
            ),
        ))
    }

    /// The stamp of one decision made for this request.
    pub fn stamp(&self, at: Timestamp, entropy: [u8; 32]) -> Stamp {
        Stamp {
            at,
            entropy,
            actor: self.actor.clone(),
            permissions: self.permissions.clone(),
            correlation: self.correlation.clone(),
            causation: self.causation,
            attempt: self.attempt,
            automation_depth: self.automation_depth,
        }
    }
}

/// `getpass.getuser()` without the password database: the first non-empty value of
/// [`GETUSER_VARIABLES`], given in that order.
pub fn getuser(values: &[Option<String>]) -> Option<&str> {
    values
        .iter()
        .flatten()
        .map(String::as_str)
        .find(|value| !value.is_empty())
}

/// A wall-clock reading in nanoseconds since the Unix epoch as a [`Timestamp`] (truncated to
/// microseconds, as Python's `datetime` holds it).
pub fn timestamp_from_unix_nanos(nanos: i128) -> Result<Timestamp, WgError> {
    let value = time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|err| WgError::workgraph(format!("clock reading out of range: {err}")))?;
    Timestamp::from_offset_datetime(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_types::ErrorCode;

    fn id(n: u32) -> Id {
        wg_model::derive_id(&[7; 32], n)
    }

    #[test]
    fn require_names_every_missing_permission() {
        let reader = BTreeSet::from([Permission::WorkspaceRead]);
        let context = RequestContext::new(ActorKind::Human, "brice", reader, id(0)).unwrap();
        assert!(context.require(&[Permission::WorkspaceRead]).is_ok());
        let error = context
            .require(&[
                Permission::ConfigManage,
                Permission::WorkspaceRead,
                Permission::TaskAccept,
            ])
            .err();
        assert_eq!(
            error.as_ref().map(|e| e.code),
            Some(ErrorCode::PermissionDenied)
        );
        assert_eq!(
            error.map(|e| e.message),
            Some("actor 'human:brice' lacks permission(s): config:manage, task:accept".to_owned())
        );
    }

    #[test]
    fn constructors_follow_python() {
        let human = RequestContext::human(None, Some("brice"), id(0)).unwrap();
        assert_eq!(human.actor.to_string(), "human:brice");
        assert!(human.permissions.contains(&Permission::TaskAccept));
        let local = RequestContext::human(None, None, id(0)).unwrap();
        assert_eq!(local.actor.to_string(), "human:local");
        let agent = RequestContext::agent("claude-code", None, id(0)).unwrap();
        assert!(!agent.permissions.contains(&Permission::TaskAccept));
        assert!(agent.permissions.contains(&Permission::TaskClaim));
        let auto = RequestContext::automation("r", BTreeSet::new(), 2, id(0)).unwrap();
        assert_eq!(auto.automation_depth, 2);
        let deeper = auto.deeper(Some(id(1)));
        assert_eq!(
            (deeper.automation_depth, deeper.causation),
            (3, Some(id(1)))
        );
    }

    #[test]
    fn stamp_carries_the_context() {
        let ctx = RequestContext::system("worker", id(0))
            .unwrap()
            .with_attempt(Some(id(2)));
        let at = timestamp_from_unix_nanos(1_761_393_600_123_456_789).unwrap();
        let stamp = ctx.stamp(at, [1; 32]);
        assert_eq!(stamp.at.format_db(), "2025-10-25T12:00:00.123456+00:00");
        assert_eq!(stamp.correlation, Correlation::from(id(0)));
        assert_eq!(stamp.attempt, Some(id(2)));
        assert_eq!(stamp.actor.to_string(), "system:worker");
    }

    #[test]
    fn getuser_takes_the_first_non_empty() {
        let values = [
            None,
            Some(String::new()),
            Some("b".to_owned()),
            Some("c".to_owned()),
        ];
        assert_eq!(getuser(&values), Some("b"));
        assert_eq!(getuser(&[None, None]), None);
    }
}
