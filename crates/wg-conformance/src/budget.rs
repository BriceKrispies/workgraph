//! The test budget. Every run of scenarios (`validate`, `diff`, `record`, `check`) has one hard
//! wall-clock budget. It defaults to [`CEILING`] (120 s) and `--budget` cannot raise it.
//!
//! When the budget is spent the run stops starting scenarios, kills the processes and servers it
//! is waiting on, marks every scenario it did not finish as failed with [`Budget::problem`], prints
//! a final line and exits non-zero. The waits learn about the cut through the armed budget
//! ([`arm`], [`cut`]); a [`watchdog`] ends the process if a run does not wind down on its own.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The most any run may take, and the default budget. It is not configurable upward.
pub const CEILING: Duration = Duration::from_secs(120);

/// How long past the budget the watchdog lets a run wind down before it ends the process.
pub const GRACE: Duration = Duration::from_secs(2);

/// Parse a `--budget` value in seconds (fractions allowed). It must be positive and no more than
/// [`CEILING`].
pub fn parse(text: &str) -> Result<Duration, String> {
    let secs = match text.parse::<f64>() {
        Ok(secs) if secs.is_finite() && secs > 0.0 => secs,
        _ => {
            return Err(format!(
                "--budget needs a positive number of seconds, not `{text}`"
            ));
        }
    };
    if secs > CEILING.as_secs_f64() {
        return Err(format!(
            "--budget `{text}` is above the 120 s ceiling; a test run may not take longer than 120 s"
        ));
    }
    Duration::try_from_secs_f64(secs).map_err(|e| format!("--budget `{text}`: {e}"))
}

/// A wall-clock budget that started at [`Budget::start`].
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    started: Instant,
    limit: Duration,
}

impl Budget {
    /// A budget of `limit`, starting now.
    pub fn start(limit: Duration) -> Self {
        Self {
            started: Instant::now(),
            limit,
        }
    }

    /// The budget's length.
    pub fn limit(&self) -> Duration {
        self.limit
    }

    /// Time since the budget started.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Whether the budget is spent now.
    pub fn expired(&self) -> bool {
        self.expired_at(Instant::now())
    }

    /// Whether the budget is spent at `now`: the limit has been reached or passed.
    pub fn expired_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.limit
    }

    /// The problem text a scenario carries when the budget cut it.
    pub fn problem(&self) -> String {
        format!("test budget of {} s exceeded", self.limit.as_secs_f64())
    }
}

static ARMED: OnceLock<Budget> = OnceLock::new();

/// Arm the process-wide budget. The first call wins; the waits consult it through [`cut`].
pub fn arm(budget: Budget) {
    let _ = ARMED.set(budget);
}

/// Whether the armed budget is spent. Always false before [`arm`].
pub fn cut() -> bool {
    ARMED.get().is_some_and(Budget::expired)
}

/// The error a wait returns when the budget cut it.
pub fn cut_error() -> String {
    ARMED
        .get()
        .map_or_else(|| Budget::start(CEILING).problem(), Budget::problem)
}

/// Start a thread that ends the process once the budget and [`GRACE`] have passed. A run that
/// winds down on its own exits first; this only catches a run stuck where no wait can see the cut.
pub fn watchdog(budget: Budget) {
    std::thread::spawn(move || {
        let end = budget.started + budget.limit + GRACE;
        std::thread::sleep(end.saturating_duration_since(Instant::now()));
        eprintln!(
            "{}: the run did not wind down after the cut; ending it",
            budget.problem()
        );
        std::process::exit(1);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_capped_at_the_ceiling() {
        assert_eq!(parse("120").unwrap(), CEILING);
        assert_eq!(parse("120.0").unwrap(), CEILING);
        assert_eq!(parse("2.5").unwrap(), Duration::from_millis(2500));
        assert_eq!(parse("30").unwrap(), Duration::from_secs(30));
        for above in ["120.001", "121", "1e3", "3600"] {
            let err = parse(above).unwrap_err();
            assert!(err.contains("ceiling"), "{above}: {err}");
        }
        for bad in ["0", "-5", "soon", "inf", "nan", ""] {
            assert!(parse(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn accounting_counts_from_the_start_and_expires_at_the_limit() {
        let start = Instant::now();
        let budget = Budget {
            started: start,
            limit: CEILING,
        };
        assert!(!budget.expired_at(start));
        assert!(!budget.expired_at(start + Duration::from_secs(119)));
        assert!(budget.expired_at(start + CEILING));
        assert!(budget.expired_at(start + Duration::from_secs(121)));
        // A clock reading before the start is not elapsed time.
        assert!(!budget.expired_at(start.checked_sub(Duration::from_secs(1)).unwrap_or(start)));
        assert_eq!(budget.limit(), CEILING);
    }

    #[test]
    fn a_zero_budget_is_spent_at_once_and_names_its_limit() {
        let budget = Budget::start(Duration::ZERO);
        assert!(budget.expired());
        assert_eq!(budget.problem(), "test budget of 0 s exceeded");
        assert_eq!(
            Budget::start(CEILING).problem(),
            "test budget of 120 s exceeded"
        );
        assert_eq!(
            Budget::start(Duration::from_millis(2500)).problem(),
            "test budget of 2.5 s exceeded"
        );
    }
}
