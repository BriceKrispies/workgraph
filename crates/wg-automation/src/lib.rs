//! wg-automation: rule catalog, condition matching, templates, preview, depth and rate guards,
//! and cron (`automation/cron.py`, `automation/rules.py`, the guards of `automation/engine.py`).
//!
//! Core tier: every function is a pure function of its arguments. Time arrives as a parameter,
//! and nothing reads a clock, the database or the environment. Python value semantics that the
//! output depends on (`repr`, `==`, `in`, truthiness) live in [`py`].

pub mod condition;
pub mod cron;
pub mod guard;
pub mod plan;
pub mod py;
pub mod rules;
pub mod spec;
pub mod template;
