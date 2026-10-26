//! The pure rules of `WorktreeManager.prepare` (WG-3773): the lease `acquire_lease` takes, the
//! check lists a manifest carries, and the three key sets `worktree.created` is written with.

use rpds::RedBlackTreeSetSync;
use serde_json::{Value, json};
use wg_model::entity::{JsonObject, ResourceLease};
use wg_model::event::WorktreeCreated;
use wg_model::stamp::Stamp;
use wg_model::write::{RowKey, Write};
use wg_runtimespec::worktrees::{acquire_lease_decision, prepare_checks};
use wg_types::{Actor, ErrorCode, Id, Timestamp, WgError};
use wg_verify::{ApplicableCheck, CheckSpec};

const RESOURCE: &str = "checkout:c1";

fn stamp_at(second: u8) -> Result<Stamp, WgError> {
    Ok(Stamp {
        at: Timestamp::from_components(2026, 10, 26, 9, 0, second, 0)?,
        entropy: [7; 32],
        actor: Actor::parse("human:brice")?,
        permissions: RedBlackTreeSetSync::new_sync(),
        correlation: Id::from_u128(99).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    })
}

/// A lease held by `holder`, expiring at `expires_at` (a timestamp of the stamp clock).
fn lease(holder: &str, expires_at: Option<Timestamp>) -> Result<ResourceLease, WgError> {
    Ok(ResourceLease {
        resource_key: RESOURCE.to_owned(),
        holder: holder.to_owned(),
        purpose: "in-place write WG-1".to_owned(),
        acquired_at: Timestamp::from_components(2026, 10, 26, 8, 0, 0, 0)?,
        expires_at,
    })
}

/// The row each write of a decision touches, in order.
fn keys<'a>(writes: impl IntoIterator<Item = &'a Write>) -> Vec<RowKey> {
    writes.into_iter().map(Write::key).collect()
}

fn object(value: Value) -> Result<JsonObject, String> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!("expected an object, got {other}")),
    }
}

fn written_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn a_free_resource_takes_a_new_lease() -> Result<(), WgError> {
    let stamp = stamp_at(0)?;
    let decision =
        acquire_lease_decision(None, RESOURCE, "attempt-1", "in-place write WG-1", &stamp)?;
    assert_eq!(
        keys(&decision.writes),
        vec![RowKey::ResourceLease(RESOURCE.to_owned())]
    );
    Ok(())
}

#[test]
fn the_holder_of_a_live_lease_keeps_it_with_a_fresh_expiry() -> Result<(), WgError> {
    let stamp = stamp_at(0)?;
    let held = lease(
        "attempt-1",
        Some(Timestamp::from_components(2026, 10, 26, 9, 5, 0, 0)?),
    )?;
    let decision = acquire_lease_decision(
        Some(&held),
        RESOURCE,
        "attempt-1",
        "in-place write WG-1",
        &stamp,
    )?;
    assert_eq!(
        keys(&decision.writes),
        vec![RowKey::ResourceLease(RESOURCE.to_owned())]
    );
    Ok(())
}

#[test]
fn another_holder_of_a_live_lease_makes_the_resource_busy() -> Result<(), WgError> {
    let stamp = stamp_at(0)?;
    let held = lease(
        "attempt-2",
        Some(Timestamp::from_components(2026, 10, 26, 9, 5, 0, 0)?),
    )?;
    let refused = acquire_lease_decision(
        Some(&held),
        RESOURCE,
        "attempt-1",
        "in-place write WG-1",
        &stamp,
    );
    match refused {
        Err(err) => assert_eq!(err.code, ErrorCode::ResourceBusy),
        Ok(_) => {
            return Err(WgError::new(
                ErrorCode::WorkgraphError,
                "a live lease was taken over",
            ));
        }
    }
    Ok(())
}

#[test]
fn a_lease_without_an_expiry_is_busy_for_other_holders() -> Result<(), WgError> {
    let stamp = stamp_at(0)?;
    let held = lease("attempt-2", None)?;
    assert!(acquire_lease_decision(Some(&held), RESOURCE, "attempt-1", "p", &stamp).is_err());
    Ok(())
}

#[test]
fn an_expired_lease_is_taken_over() -> Result<(), WgError> {
    let stamp = stamp_at(0)?;
    let held = lease(
        "attempt-2",
        Some(Timestamp::from_components(2026, 10, 26, 8, 30, 0, 0)?),
    )?;
    let decision = acquire_lease_decision(Some(&held), RESOURCE, "attempt-1", "p", &stamp)?;
    assert_eq!(
        keys(&decision.writes),
        vec![RowKey::ResourceLease(RESOURCE.to_owned())]
    );
    Ok(())
}

#[test]
fn prepare_shape_of_worktree_created_keeps_null_branch_and_base() -> Result<(), String> {
    let payload = WorktreeCreated {
        purpose: None,
        stack_id: None,
        repository: "api".to_owned(),
        path: "/data/worktrees/a/api".to_owned(),
        base_sha: "abc".to_owned(),
        base: None,
        branch: None,
    };
    let data = serde_json::to_value(&payload).map_err(|err| err.to_string())?;
    let mut found = written_keys(&data);
    found.sort();
    assert_eq!(found, ["base", "base_sha", "branch", "path", "repository"]);
    assert_eq!(data["branch"], Value::Null);
    assert_eq!(data["base"], Value::Null);
    Ok(())
}

#[test]
fn stack_integration_shape_has_branch_and_no_base() -> Result<(), String> {
    let payload = WorktreeCreated {
        purpose: Some("stack_integration".to_owned()),
        stack_id: Some(Id::from_u128(5)),
        repository: "api".to_owned(),
        path: "/data/worktrees/stacks/s/api".to_owned(),
        base_sha: "abc".to_owned(),
        base: None,
        branch: Some("wg/stack/s".to_owned()),
    };
    let data = serde_json::to_value(&payload).map_err(|err| err.to_string())?;
    let text = serde_json::to_string(&data).map_err(|err| err.to_string())?;
    assert_eq!(
        text,
        format!(
            "{{\"purpose\":\"stack_integration\",\"stack_id\":\"{}\",\"repository\":\"api\",\"path\":\"/data/worktrees/stacks/s/api\",\"branch\":\"wg/stack/s\",\"base_sha\":\"abc\"}}",
            Id::from_u128(5)
        )
    );
    Ok(())
}

#[test]
fn stack_bisect_shape_has_neither_branch_nor_base() -> Result<(), String> {
    let payload = WorktreeCreated {
        purpose: Some("stack_bisect".to_owned()),
        stack_id: Some(Id::from_u128(6)),
        repository: "api".to_owned(),
        path: "/data/worktrees/stacks/s/api".to_owned(),
        base_sha: "abc".to_owned(),
        base: None,
        branch: None,
    };
    let data = serde_json::to_value(&payload).map_err(|err| err.to_string())?;
    assert_eq!(
        written_keys(&data).len(),
        5,
        "purpose, stack_id, repository, path, base_sha"
    );
    assert!(data.get("branch").is_none());
    assert!(data.get("base").is_none());
    Ok(())
}

fn applicable(name: &str, tier: &str) -> Result<ApplicableCheck, String> {
    let raw: JsonObject = object(json!({"name": name, "command": ["pytest", "-q"], "tier": tier}))?;
    Ok(ApplicableCheck {
        config_id: Id::from_u128(1),
        config_name: "ci".to_owned(),
        scope_type: "workspace".to_owned(),
        scope_id: Id::from_u128(2),
        spec: CheckSpec::from_json(&raw).map_err(|err| err.message)?,
    })
}

#[test]
fn the_manifest_lists_every_check_and_the_fast_ones_for_the_agent() -> Result<(), String> {
    let checks = [applicable("lint", "fast")?, applicable("unit", "full")?];
    let (verification, agent_checks) = prepare_checks(&checks);
    assert_eq!(verification.len(), 2);
    assert_eq!(verification[1]["name"], "unit");
    assert_eq!(verification[1]["tier"], "full");
    assert_eq!(
        agent_checks,
        vec![json!({"name": "lint", "command": ["pytest", "-q"], "repository": null})]
    );
    Ok(())
}
