//! Band staircase: the pure state machine of `src/workgraph/experiments/staircase.py` (CORE-25).
//!
//! Trials are grouped into consecutive visits of `per_visit` trials at one band. A visit passes
//! when at least half of its trials pass; a pass moves the band up, a failure moves it down, both
//! clamped to the band range. The staircase ends after enough direction reversals or trials. The
//! threshold is the mean band of the reversal visits, and the refine phase spends its trials on the
//! bands around that threshold. [`next_step`] recomputes everything from the history.
//!
//! Protocol mappings arrive already decoded into [`ProtocolFields`]; the shell reads the JSON and
//! reports absent keys and non-integer values, and [`StaircaseConfig::from_protocol`] applies the
//! same validation and messages as the Python module.

use std::collections::BTreeSet;

use crate::stats::ValueError;

/// A decoded protocol value: a Python `bool`, an `int`, or any other value carried as its `repr()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Raw {
    /// A Python `bool` (rejected as an integer field).
    Bool(bool),
    /// A Python `int`.
    Int(i64),
    /// Any other value, as the `repr()` the Python error message would show.
    Other(String),
}

/// A required protocol key: present with its value, or absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The key is not in the mapping.
    Absent,
    /// The key is present.
    Found(Raw),
}

/// The protocol keys the staircase reads, in the order Python reads them.
///
/// `has_bands` and `has_staircase` report whether `protocol["bands"]` and `protocol["staircase"]`
/// exist; `min`..`max_trials` are the keys inside them. `refine_*` are `None` when the `refine`
/// mapping or the key is absent, which takes the default (`0` trials, width `1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolFields {
    /// Whether `protocol["bands"]` exists.
    pub has_bands: bool,
    /// Whether `protocol["staircase"]` exists.
    pub has_staircase: bool,
    /// `bands.min`.
    pub min: Lookup,
    /// `bands.max`.
    pub max: Lookup,
    /// `staircase.per_visit`.
    pub per_visit: Lookup,
    /// `staircase.up`.
    pub up: Lookup,
    /// `staircase.down`.
    pub down: Lookup,
    /// `staircase.max_reversals`.
    pub max_reversals: Lookup,
    /// `staircase.max_trials`.
    pub max_trials: Lookup,
    /// `refine.trials`, if given.
    pub refine_trials: Option<Raw>,
    /// `refine.width`, if given.
    pub refine_width: Option<Raw>,
}

/// Band range and stopping rules for one staircase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaircaseConfig {
    /// Lowest band (at least 1).
    pub min_band: i64,
    /// Highest band (at least `min_band`).
    pub max_band: i64,
    /// Trials per visit (at least 1).
    pub per_visit: i64,
    /// Bands moved up after a passing visit (at least 1).
    pub up: i64,
    /// Bands moved down after a failing visit (at least 1).
    pub down: i64,
    /// Direction reversals that end the staircase (at least 1).
    pub max_reversals: i64,
    /// Trials that end the staircase (at least `per_visit`).
    pub max_trials: i64,
    /// Refine trials (at least 0).
    pub refine_trials: i64,
    /// Half-width of the refine window (at least 0).
    pub refine_width: i64,
}

/// Which phase a [`Step`] belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The staircase walks the bands.
    Staircase,
    /// The refine phase samples the bands around the threshold.
    Refine,
    /// Both phases are finished.
    Done,
}

impl Phase {
    /// The phase name used in the protocol and the trial records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staircase => "staircase",
            Self::Refine => "refine",
            Self::Done => "done",
        }
    }
}

/// What to do next: the phase, the band for the next trial, and the state behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The phase of the next trial, or `Done`.
    pub phase: Phase,
    /// The band for the next trial; `None` when done.
    pub band: Option<i64>,
    /// Number of direction reversals seen.
    pub reversals: usize,
    /// Mean band of the reversals, else the last completed visit's band; `None` before any visit.
    pub threshold: Option<f64>,
    /// Completed visits as `(band, passes, trials)`, in order.
    pub visits: Vec<(i64, i64, i64)>,
}

/// One visit: its band, passes and trials, and whether it filled `per_visit` trials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Visit {
    band: i64,
    passes: i64,
    trials: i64,
    complete: bool,
}

impl Visit {
    /// A visit passes when at least half of its trials pass.
    fn passed(self) -> bool {
        self.passes * 2 >= self.trials
    }
}

fn invalid<T>(message: String) -> Result<T, ValueError> {
    Err(ValueError::new(message))
}

/// `_int_field`: an integer of at least `minimum`, rejecting Python `bool`s.
fn int_field(raw: &Raw, name: &str, minimum: i64) -> Result<i64, ValueError> {
    match raw {
        Raw::Bool(flag) => invalid(format!(
            "{name} must be an integer, got {}",
            if *flag { "True" } else { "False" }
        )),
        Raw::Other(repr) => invalid(format!("{name} must be an integer, got {repr}")),
        Raw::Int(value) => at_least(name, *value, minimum).map(|()| *value),
    }
}

fn at_least(name: &str, value: i64, minimum: i64) -> Result<(), ValueError> {
    if value < minimum {
        invalid(format!("{name} must be >= {minimum}, got {value}"))
    } else {
        Ok(())
    }
}

/// A required key: the value, or the Python `KeyError` message.
fn required<'a>(lookup: &'a Lookup, name: &str) -> Result<&'a Raw, ValueError> {
    match lookup {
        Lookup::Found(raw) => Ok(raw),
        Lookup::Absent => invalid(format!("protocol is missing a required field: '{name}'")),
    }
}

fn group_visits(
    config: &StaircaseConfig,
    history: &[(i64, bool)],
) -> Result<Vec<Visit>, ValueError> {
    let per_visit = usize::try_from(config.per_visit).unwrap_or(1).max(1);
    history
        .chunks(per_visit)
        .enumerate()
        .map(|(index, chunk)| {
            let bands: BTreeSet<i64> = chunk.iter().map(|(band, _)| *band).collect();
            if bands.len() > 1 {
                let sorted: Vec<i64> = bands.into_iter().collect();
                return invalid(format!(
                    "visit starting at trial {} mixes bands {sorted:?}",
                    index * per_visit
                ));
            }
            Ok(Visit {
                band: chunk.first().map_or(0, |(band, _)| *band),
                passes: i64::try_from(chunk.iter().filter(|(_, passed)| *passed).count())
                    .unwrap_or(0),
                trials: i64::try_from(chunk.len()).unwrap_or(0),
                complete: chunk.len() == per_visit,
            })
        })
        .collect()
}

/// Completed visits whose direction differs from the previous completed visit's direction.
fn reversal_visits(completed: &[Visit]) -> Vec<Visit> {
    completed
        .iter()
        .zip(std::iter::once(None).chain(completed.iter().map(|visit| Some(visit.passed()))))
        .filter(|(visit, previous)| previous.is_some_and(|prev| prev != visit.passed()))
        .map(|(visit, _)| *visit)
        .collect()
}

fn staircase_ended(config: &StaircaseConfig, trials: usize, reversals: usize) -> bool {
    let trials = i64::try_from(trials).unwrap_or(i64::MAX);
    let reversals = i64::try_from(reversals).unwrap_or(i64::MAX);
    let at_boundary = trials % config.per_visit == 0;
    trials >= config.max_trials || (at_boundary && reversals >= config.max_reversals)
}

fn current_band(config: &StaircaseConfig, visits: &[Visit]) -> i64 {
    match visits.last() {
        None => config.max_band,
        Some(last) if !last.complete => last.band,
        Some(last) => {
            let delta = if last.passed() {
                config.up
            } else {
                -config.down
            };
            (last.band + delta)
                .max(config.min_band)
                .min(config.max_band)
        }
    }
}

/// The refine band: the least-tried band of the window around the threshold, ties to the closest
/// band and then the lower one. `round()` ties go to the even neighbour, as Python's `round`.
fn refine_band(
    config: &StaircaseConfig,
    refine: &[(i64, bool)],
    threshold: f64,
) -> Result<i64, ValueError> {
    let center = threshold.round_ties_even() as i64;
    let low = (center - config.refine_width).max(config.min_band);
    let high = (center + config.refine_width).min(config.max_band);
    (low..=high)
        .map(|band| {
            let count = refine.iter().filter(|(b, _)| *b == band).count();
            (count, (band as f64 - threshold).abs(), band)
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)))
        .map(|(_, _, band)| band)
        .ok_or_else(|| ValueError::new("min() arg is an empty sequence"))
}

/// Return the next phase and band given the ordered trial history of each phase.
///
/// # Errors
///
/// Returns a [`ValueError`] when a visit mixes bands, when refine trials are given before the
/// staircase has ended, or when the refine window is empty (Python's `min()` message).
pub fn next_step(
    config: &StaircaseConfig,
    staircase: &[(i64, bool)],
    refine: &[(i64, bool)],
) -> Result<Step, ValueError> {
    let visits = group_visits(config, staircase)?;
    let completed: Vec<Visit> = visits
        .iter()
        .copied()
        .filter(|visit| visit.complete)
        .collect();
    let reversals = reversal_visits(&completed);
    let ended = staircase_ended(config, staircase.len(), reversals.len());
    if !refine.is_empty() && !ended {
        return invalid("refine trials were given before the staircase phase finished".to_string());
    }

    let threshold = if reversals.is_empty() {
        completed.last().map(|visit| visit.band as f64)
    } else {
        let total: i64 = reversals.iter().map(|visit| visit.band).sum();
        Some(total as f64 / reversals.len() as f64)
    };
    let visit_summary: Vec<(i64, i64, i64)> = completed
        .iter()
        .map(|visit| (visit.band, visit.passes, visit.trials))
        .collect();
    let reversal_count = reversals.len();

    if !ended {
        return Ok(Step {
            phase: Phase::Staircase,
            band: Some(current_band(config, &visits)),
            reversals: reversal_count,
            threshold,
            visits: visit_summary,
        });
    }

    let refine_left = i64::try_from(refine.len()).is_ok_and(|n| n < config.refine_trials);
    match threshold {
        Some(center) if refine_left => Ok(Step {
            phase: Phase::Refine,
            band: Some(refine_band(config, refine, center)?),
            reversals: reversal_count,
            threshold,
            visits: visit_summary,
        }),
        _ => Ok(Step {
            phase: Phase::Done,
            band: None,
            reversals: reversal_count,
            threshold,
            visits: visit_summary,
        }),
    }
}

impl StaircaseConfig {
    /// Whether `band` lies in the protocol range `min_band..=max_band`, the bounds inclusive. A
    /// degenerate range (`min_band == max_band`) holds exactly one band.
    #[must_use]
    pub fn contains(&self, band: i64) -> bool {
        (self.min_band..=self.max_band).contains(&band)
    }

    /// Checks the ranges and cross-field rules of `StaircaseConfig.__post_init__`.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] naming the first field out of range or the broken rule.
    pub fn validate(self) -> Result<Self, ValueError> {
        at_least("min_band", self.min_band, 1)?;
        at_least("max_band", self.max_band, 1)?;
        if self.min_band > self.max_band {
            return invalid(format!(
                "min_band ({}) must be <= max_band ({})",
                self.min_band, self.max_band
            ));
        }
        at_least("per_visit", self.per_visit, 1)?;
        at_least("up", self.up, 1)?;
        at_least("down", self.down, 1)?;
        at_least("max_reversals", self.max_reversals, 1)?;
        at_least("max_trials", self.max_trials, 1)?;
        if self.max_trials < self.per_visit {
            return invalid(format!(
                "max_trials ({}) must be >= per_visit ({})",
                self.max_trials, self.per_visit
            ));
        }
        at_least("refine_trials", self.refine_trials, 0)?;
        at_least("refine_width", self.refine_width, 0)?;
        Ok(self)
    }

    /// Build the config from a decoded protocol, as `StaircaseConfig.from_protocol` does.
    ///
    /// Missing keys are reported first, in the order Python reads them; then each integer is
    /// checked with its protocol path (`bands.min`, `staircase.per_visit`, ...).
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] for a missing key, a non-integer or out-of-range value, or a
    /// rule broken by [`StaircaseConfig::validate`].
    pub fn from_protocol(protocol: &ProtocolFields) -> Result<Self, ValueError> {
        if !protocol.has_bands {
            return invalid("protocol is missing a required field: 'bands'".to_string());
        }
        if !protocol.has_staircase {
            return invalid("protocol is missing a required field: 'staircase'".to_string());
        }
        let min = required(&protocol.min, "min")?;
        let max = required(&protocol.max, "max")?;
        let per_visit = required(&protocol.per_visit, "per_visit")?;
        let up = required(&protocol.up, "up")?;
        let down = required(&protocol.down, "down")?;
        let max_reversals = required(&protocol.max_reversals, "max_reversals")?;
        let max_trials = required(&protocol.max_trials, "max_trials")?;
        let refine_trials = protocol.refine_trials.clone().unwrap_or(Raw::Int(0));
        let refine_width = protocol.refine_width.clone().unwrap_or(Raw::Int(1));
        Self {
            min_band: int_field(min, "bands.min", 1)?,
            max_band: int_field(max, "bands.max", 1)?,
            per_visit: int_field(per_visit, "staircase.per_visit", 1)?,
            up: int_field(up, "staircase.up", 1)?,
            down: int_field(down, "staircase.down", 1)?,
            max_reversals: int_field(max_reversals, "staircase.max_reversals", 1)?,
            max_trials: int_field(max_trials, "staircase.max_trials", 1)?,
            refine_trials: int_field(&refine_trials, "refine.trials", 0)?,
            refine_width: int_field(&refine_width, "refine.width", 0)?,
        }
        .validate()
    }
}
