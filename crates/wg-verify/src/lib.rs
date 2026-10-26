//! wg-verify: check configs, check selection by scope and tier, failure signatures, run outcome,
//! acceptance and auto-accept.
//!
//! Every function here is a pure decision over values: stored check objects, configuration rows,
//! the facts a shell gathered, and log lines. Process execution, log files, the database and the
//! clock stay in the shell (docs/rust-rewrite.md section 4.6). The golden vectors of GV-02
//! (`golden/signatures/`) and the unit ports of `test_failure_signatures.py` and
//! `test_verification_tiers.py` pin the behaviour.

mod acceptance;
mod config;
mod outcome;
mod scope;
mod signatures;

pub use acceptance::{
    AcceptRefusal, AutoAcceptFacts, AutoAcceptSkip, LatestRun, accept_refusal, auto_accept_skip,
    latest_run,
};
pub use config::{Baseline, CheckSpec, CheckTier};
pub use outcome::{
    CheckResult, ProcessOutcome, RunOutcome, check_signatures, check_status, is_fast_pass,
    ran_fast_tier_only, run_outcome,
};
pub use scope::{
    ApplicableCheck, TaskScope, applicable_checks, applicable_configs, checks_for_tier, parse_tier,
};
pub use signatures::{FailureParser, extract_failure_signatures};
