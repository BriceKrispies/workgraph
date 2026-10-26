//! wg-stacks: stack membership, integration queue, verification, landing.
//!
//! Grows by domain task. The CORE-14 modules hold the pure decisions of `core/policies/stacks.py`,
//! the membership rules of `core/services/stacks.py`, and the ejection rules of
//! `integrations/git/stack_verification.py`. Database reads, git refs and job queues stay in the
//! shell crates that call these functions.

pub mod ejection;
pub mod item;
pub mod membership;
