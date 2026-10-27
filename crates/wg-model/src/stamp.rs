//! [`Stamp`]: the clock, entropy and request context of one decision, and the id derivation of
//! section 4.1 ([`derive_id`], [`IdSeq`]).
//!
//! The shell reads the clock once and draws 32 bytes from the OS once per decision, and writes
//! them here with the acting identity (Python's `RequestContext`). Core code never reads a clock
//! or an RNG: every timestamp it writes is `stamp.at`, every new id comes from the stamp.

use rpds::RedBlackTreeSetSync;
use uuid::Builder;
use wg_types::{Actor, Correlation, Id, Permission, Timestamp, WgError};

/// Written by the shell for every decision; the only source of time, ids and the actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The decision's clock reading, truncated to microseconds.
    pub at: Timestamp,
    /// 32 bytes from the OS RNG; every id of the decision is derived from them.
    pub entropy: [u8; 32],
    /// The acting identity (`ctx.actor`, `ctx.actor_kind`).
    pub actor: Actor,
    /// The permissions the actor holds (`ctx.permissions`).
    pub permissions: RedBlackTreeSetSync<Permission>,
    /// `ctx.correlation_id`: shared by every event of one request.
    pub correlation: Correlation,
    /// `ctx.causation_id`: the event that caused this request, if any.
    pub causation: Option<Id>,
    /// `ctx.attempt_id`: the attempt the request runs under, if any.
    pub attempt: Option<Id>,
    /// `ctx.automation_depth`: 0 outside automation runs.
    pub automation_depth: u32,
}

impl Stamp {
    /// The id in `slot` (see [`derive_id`]).
    pub fn id(&self, slot: u32) -> Id {
        derive_id(&self.entropy, slot)
    }

    /// A fresh id sequence starting at slot 0.
    pub fn ids(&self) -> IdSeq {
        IdSeq {
            entropy: self.entropy,
            next: 0,
        }
    }

    /// Whether the actor holds `permission`.
    pub fn has(&self, permission: Permission) -> bool {
        self.permissions.contains(&permission)
    }

    /// `security.permissions.require`: fails with `permission_denied` naming every missing
    /// permission, sorted by wire string, with the same message and details as Python.
    pub fn require(&self, permissions: &[Permission]) -> Result<(), WgError> {
        let missing: RedBlackTreeSetSync<&'static str> = permissions
            .iter()
            .filter(|permission| !self.has(**permission))
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
}

/// A position in a decision's id sequence. Threaded by value through helpers: each call that
/// needs ids takes an `IdSeq` and returns the advanced one, so the slot of every id is a pure
/// function of the order of the calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdSeq {
    entropy: [u8; 32],
    next: u32,
}

impl IdSeq {
    /// The next id and the advanced sequence.
    #[must_use]
    pub fn next(self) -> (Id, IdSeq) {
        (
            derive_id(&self.entropy, self.next),
            IdSeq {
                next: self.next.wrapping_add(1),
                ..self
            },
        )
    }

    /// The next `N` ids and the advanced sequence.
    #[must_use]
    pub fn take<const N: usize>(self) -> ([Id; N], IdSeq) {
        let ids = core::array::from_fn(|offset| {
            derive_id(
                &self.entropy,
                self.next
                    .wrapping_add(u32::try_from(offset).unwrap_or(u32::MAX)),
            )
        });
        let advanced = u32::try_from(N).unwrap_or(u32::MAX);
        (
            ids,
            IdSeq {
                next: self.next.wrapping_add(advanced),
                ..self
            },
        )
    }

    /// The slot the next id will take.
    pub fn position(self) -> u32 {
        self.next
    }
}

/// Derives the id in `slot` from a decision's entropy: a UUIDv4 layout (version and variant
/// bits set) whose other 122 bits are a SplitMix64 mix of the entropy and the slot. The same
/// entropy and slot always give the same id; different slots give different ids except with
/// negligible probability.
pub fn derive_id(entropy: &[u8; 32], slot: u32) -> Id {
    let even = u64::from(slot) << 1;
    let high = mix64(word(entropy, 0) ^ mix64(word(entropy, 2) ^ even));
    let low = mix64(word(entropy, 1) ^ mix64(word(entropy, 3) ^ (even | 1)));
    let bits = (u128::from(high) << 64) | u128::from(low);
    Builder::from_random_bytes(bits.to_be_bytes()).into_uuid()
}

/// Little-endian 64-bit word `index` (0 to 3) of the entropy.
fn word(entropy: &[u8; 32], index: usize) -> u64 {
    entropy
        .iter()
        .skip(index * 8)
        .take(8)
        .rev()
        .fold(0, |acc, byte| (acc << 8) | u64::from(*byte))
}

/// The SplitMix64 finaliser: a bijection on `u64` with full avalanche.
const fn mix64(value: u64) -> u64 {
    let value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Python's `repr()` of a `str`, now owned by `wg-types` so the permission errors can use it.
pub use wg_types::py_repr_str;
