//! Triage-apply validation and plan (CORE-05). Mirrors `core/services/triage.py`.
//!
//! A triage file is a JSON array of verdicts. [`plan_triage`] validates the whole array before
//! anything changes, resolves each verdict to its task, and returns the steps the shell runs, in
//! order, through the task services. A step that creates a follow-up task produces a new key; the
//! shell collects those keys in order and passes them to [`Superseded::comment`] and
//! [`Superseded::reason`], which is why the superseded text is rendered from its parts.
//!
//! A `needs-decision` verdict whose `decision` is an object yields a [`Step::RequestDecision`]
//! holding the question and the normalised option objects. The decision record itself belongs to
//! `decisions` (WG-3690); this module does not create one.

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use wg_types::{ErrorCode, Id, TaskPriority, TaskStatus, WgError};

use crate::lifecycle::check_task_transition;

pub const TRIAGE_TAG_PREFIX: &str = "triage:";
pub const KIND_TAG_PREFIX: &str = "kind:";
pub const DROPPED_TAG: &str = "wg136";
/// Rewrite sizes: what a rewrite touches, never an effort estimate.
pub const SIZES: &[&str] = &["one-file", "one-crate", "several-crates"];
/// Limits of a decision request (`DecisionRequest` and `DecisionOptionIn` in `schemas.py`).
const QUESTION_MAX_CHARS: usize = 5000;
const OPTIONS_MAX: usize = 26;
const OPTION_LABEL_MAX_CHARS: usize = 500;
const OPTION_ID_MAX_CHARS: usize = 64;
const OPTION_DETAIL_MAX_CHARS: usize = 5000;
/// Limits of a file spec's path and anchor (`TaskFileIn`).
const FILE_PATH_MAX_CHARS: usize = 1000;
const FILE_ANCHOR_MAX_CHARS: usize = 300;

/// The verdict vocabulary of a triage file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Resolved,
    Obsolete,
    Open,
    NeedsDecision,
    NeedsRun,
    Unclear,
}

impl Verdict {
    /// The wire string of this verdict.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Obsolete => "obsolete",
            Self::Open => "open",
            Self::NeedsDecision => "needs-decision",
            Self::NeedsRun => "needs-run",
            Self::Unclear => "unclear",
        }
    }

    /// Parses a wire string; `None` when it names no verdict.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "resolved" => Some(Self::Resolved),
            "obsolete" => Some(Self::Obsolete),
            "open" => Some(Self::Open),
            "needs-decision" => Some(Self::NeedsDecision),
            "needs-run" => Some(Self::NeedsRun),
            "unclear" => Some(Self::Unclear),
            _ => None,
        }
    }

    /// Verdicts that move the task to cancelled.
    pub const fn cancels(self) -> bool {
        matches!(self, Self::Resolved | Self::Obsolete)
    }

    /// Verdicts that leave the task waiting: a later file may replace them.
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::NeedsDecision | Self::NeedsRun | Self::Unclear)
    }

    /// The report action this verdict produces.
    pub const fn action(self) -> Action {
        if self.cancels() {
            Action::Cancelled
        } else if matches!(self, Self::Open) {
            Action::Superseded
        } else {
            Action::Commented
        }
    }
}

/// What a planned verdict does to its task, as the report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Cancelled,
    Superseded,
    Commented,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Superseded => "superseded",
            Self::Commented => "commented",
        }
    }
}

/// A file a task expects to change: a path, optionally narrowed to an anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub path: String,
    pub anchor: Option<String>,
}

/// One rewrite of an `open` verdict, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub title: String,
    pub description: String,
    /// The raw file specs as written; they are parsed into [`FileSpec`]s when the follow-up is built.
    pub files: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    /// The size as written (a legacy size stays legacy until the tag is built).
    pub size: Option<String>,
}

/// The `decision` of a `needs-decision` verdict.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Prose: a comment only.
    Prose(String),
    /// An object: a comment and a decision request. `options` are normalised: a bare label becomes
    /// `{"label": ...}`, and every option object is kept as written.
    Structured {
        question: String,
        options: Vec<Value>,
    },
}

/// A verdict whose shape passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The key as written in the file.
    pub key: String,
    pub verdict: Verdict,
    pub summary: String,
    pub evidence: Vec<String>,
    pub rewrites: Vec<Rewrite>,
    pub duplicates: Vec<String>,
    pub decision: Option<Decision>,
    pub command: Option<String>,
}

/// The task fields the plan reads, resolved by the shell from the key.
#[derive(Debug, Clone, PartialEq)]
pub struct TriageTarget {
    pub key: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    /// The tags, in the order the shell reads them (the first `triage:` tag is named in a skip).
    pub tags: Vec<String>,
    pub projects: Vec<Id>,
    pub repositories: Vec<Id>,
}

/// A follow-up task created from one rewrite. `follows_up` is the original task's key; the new
/// task gets a `follows_up` relation to it.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowUp {
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub priority: TaskPriority,
    pub tags: Vec<String>,
    pub files: Vec<FileSpec>,
    pub projects: Vec<Id>,
    pub repositories: Vec<Id>,
    pub follows_up: String,
}

/// The text of an `open` verdict's comment and cancel reason, rendered once the follow-up keys
/// are known.
#[derive(Debug, Clone, PartialEq)]
pub struct Superseded {
    pub source: Option<String>,
    pub summary: String,
    pub evidence: Vec<String>,
    /// The keys of `duplicate_of`, after the created follow-up keys.
    pub duplicates: Vec<String>,
}

impl Superseded {
    /// The keys named in the text: the created follow-ups, then the duplicates.
    fn keys(&self, created: &[String]) -> String {
        created
            .iter()
            .chain(&self.duplicates)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The comment body: `Triage (<source>): superseded by <keys>. <summary>` and the evidence.
    pub fn comment(&self, created: &[String]) -> String {
        comment_body(
            self.source.as_deref(),
            &format!("superseded by {}", self.keys(created)),
            &self.summary,
            &self.evidence,
            &[],
        )
    }

    /// The cancel reason: `triage: superseded by <keys>`.
    pub fn reason(&self, created: &[String]) -> String {
        format!("triage: superseded by {}", self.keys(created))
    }
}

/// One step of a planned verdict, run in order by the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Removes these tags (the stale `triage:` tags of an earlier verdict).
    RemoveTags(Vec<String>),
    /// Creates a follow-up task with a `follows_up` relation to the planned task.
    CreateFollowUp(FollowUp),
    /// Adds a `duplicates` relation from the planned task to `target`.
    MarkDuplicate { target: String },
    /// Adds a plain comment.
    Comment(String),
    /// Adds the superseded comment; render with [`Superseded::comment`].
    CommentSuperseded(Superseded),
    /// Adds these tags.
    AddTags(Vec<String>),
    /// Requests a decision on the planned task. `options` are validated option objects.
    RequestDecision {
        question: String,
        options: Vec<Value>,
    },
    /// Moves the task to cancelled with this reason.
    Cancel(String),
    /// Moves the task to cancelled; the reason is rendered with [`Superseded::reason`].
    CancelSuperseded(Superseded),
}

/// A verdict that passed validation and resolved to a task.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedVerdict {
    pub index: usize,
    /// The task's key as the shell resolved it.
    pub key: String,
    pub verdict: Verdict,
    pub action: Action,
    pub steps: Vec<Step>,
}

/// A verdict skipped because its task was already triaged.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub key: String,
    pub verdict: Verdict,
    pub reason: String,
}

/// A rejected element. `key` and `verdict` are the values as written (`null` when absent).
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub index: usize,
    pub key: Value,
    pub verdict: Value,
    pub error: String,
}

/// The whole plan. `planned` keeps file order; `errors` and `skipped` do too.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TriagePlan {
    pub planned: Vec<PlannedVerdict>,
    pub skipped: Vec<Skipped>,
    pub errors: Vec<Failure>,
}

/// The top level of a triage file must be a JSON array of verdicts.
pub fn verdict_list(value: &Value) -> Result<&[Value], WgError> {
    value.as_array().map(Vec::as_slice).ok_or_else(|| {
        WgError::new(
            ErrorCode::ValidationFailed,
            "triage file must contain a JSON array of verdicts",
        )
    })
}

/// Plans a triage file. `lookup` resolves a key to its task; its error (not found, a malformed
/// key) is recorded against the element. `source` names the batch in every comment.
pub fn plan_triage<F>(entries: &[Value], source: Option<&str>, lookup: F) -> TriagePlan
where
    F: Fn(&str) -> Result<TriageTarget, WgError>,
{
    let mut plan = TriagePlan::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, value) in entries.iter().enumerate() {
        let raw_key = raw_field(value, "key");
        let raw_verdict = raw_field(value, "verdict");
        let fail = |error: String| Failure {
            index,
            key: raw_key.clone(),
            verdict: raw_verdict.clone(),
            error,
        };

        let entry = match parse_entry(value) {
            Ok(entry) => entry,
            Err(problem) => {
                plan.errors.push(fail(problem));
                continue;
            }
        };
        if !seen.insert(entry.key.trim().to_uppercase()) {
            plan.errors
                .push(fail(format!("duplicate key {} in file", entry.key)));
            continue;
        }

        let target = match lookup(&entry.key) {
            Ok(target) => target,
            Err(error) => {
                plan.errors.push(fail(error.message));
                continue;
            }
        };

        let existing: Vec<&str> = target
            .tags
            .iter()
            .map(String::as_str)
            .filter(|tag| tag.starts_with(TRIAGE_TAG_PREFIX))
            .collect();
        if let Some(first) = existing.first() {
            let pending: Vec<&str> = existing
                .iter()
                .map(|tag| &tag[TRIAGE_TAG_PREFIX.len()..])
                .collect();
            let all_pending = pending
                .iter()
                .all(|name| Verdict::parse(name).is_some_and(Verdict::is_pending));
            if pending.contains(&entry.verdict.as_str()) || !all_pending {
                plan.skipped.push(Skipped {
                    key: target.key.clone(),
                    verdict: entry.verdict,
                    reason: format!("already triaged ({first})"),
                });
                continue;
            }
        }

        let cancelling = entry.verdict.cancels() || entry.verdict == Verdict::Open;
        if cancelling
            && check_task_transition(target.status, TaskStatus::Cancelled, false, false).is_err()
        {
            plan.errors.push(fail(format!(
                "cannot move {} from {} to cancelled",
                target.key, target.status
            )));
            continue;
        }

        plan.planned.push(PlannedVerdict {
            index,
            key: target.key.clone(),
            verdict: entry.verdict,
            action: entry.verdict.action(),
            steps: build_steps(&entry, &target, source),
        });
    }
    plan
}

/// A field of a verdict as written; `null` when the element is not an object or lacks it.
fn raw_field(value: &Value, name: &str) -> Value {
    value
        .as_object()
        .and_then(|object| object.get(name))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Validates one verdict's shape, in the order the Python checks run. The first problem is the
/// message.
pub fn parse_entry(value: &Value) -> Result<Entry, String> {
    let object = value.as_object().ok_or("verdict is not a JSON object")?;
    let key = match object.get("key") {
        Some(Value::String(key)) if !key.trim().is_empty() => key.clone(),
        _ => return Err("missing key".to_owned()),
    };
    let verdict = object
        .get("verdict")
        .and_then(Value::as_str)
        .and_then(Verdict::parse)
        .ok_or_else(|| {
            format!(
                "unknown verdict {}",
                repr(object.get("verdict").unwrap_or(&Value::Null))
            )
        })?;
    let evidence = match object.get("evidence") {
        None => Vec::new(),
        Some(value) => str_list(value).ok_or("evidence must be a list of strings")?,
    };
    let summary = match object.get("summary") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    };

    let mut entry = Entry {
        key,
        verdict,
        summary,
        evidence,
        rewrites: Vec::new(),
        duplicates: Vec::new(),
        decision: None,
        command: None,
    };

    if verdict == Verdict::Open {
        let rewrites = match object.get("rewrites") {
            None => Vec::new(),
            Some(Value::Array(items)) => items.clone(),
            Some(_) => return Err(OPEN_LISTS.to_owned()),
        };
        let duplicates = match object.get("duplicate_of") {
            None => Vec::new(),
            Some(value) => str_list(value).ok_or(OPEN_LISTS)?,
        };
        if rewrites.is_empty() && duplicates.is_empty() {
            return Err(
                "open verdict needs at least one rewrite or a duplicate_of task".to_owned(),
            );
        }
        for (i, rewrite) in rewrites.iter().enumerate() {
            entry.rewrites.push(parse_rewrite(i, rewrite)?);
        }
        entry.duplicates = duplicates;
    }

    if verdict == Verdict::NeedsDecision && object.contains_key("decision") {
        entry.decision = decision_of(object.get("decision").unwrap_or(&Value::Null))?;
    }

    if verdict == Verdict::NeedsRun {
        entry.command = Some(
            text_field(object.get("command"))
                .ok_or("needs-run verdict needs a command")?
                .to_owned(),
        );
    }
    Ok(entry)
}

const OPEN_LISTS: &str = "rewrites must be a list and duplicate_of a list of task keys";

fn parse_rewrite(i: usize, rewrite: &Value) -> Result<Rewrite, String> {
    let object = rewrite
        .as_object()
        .ok_or_else(|| format!("rewrite {i} is not an object"))?;
    let title = text_field(object.get("title"))
        .ok_or_else(|| format!("rewrite {i} is missing a title"))?
        .to_owned();
    let description = text_field(object.get("description"))
        .ok_or_else(|| format!("rewrite {i} is missing a description"))?
        .to_owned();
    let size = match object.get("size") {
        None | Some(Value::Null) => None,
        Some(Value::String(size)) if SIZES.contains(&legacy_size(size)) => Some(size.clone()),
        Some(other) => return Err(format!("rewrite {i} has unknown size {}", repr(other))),
    };
    let files = object.get("files").map_or(Some(Vec::new()), str_list);
    let criteria = object
        .get("acceptance_criteria")
        .map_or(Some(Vec::new()), str_list);
    let (Some(files), Some(criteria)) = (files, criteria) else {
        return Err(format!(
            "rewrite {i} files and acceptance_criteria must be lists of strings"
        ));
    };
    if let Some(bad) = files
        .iter()
        .find(|spec| !spec.trim().is_empty() && !is_file_spec(spec))
    {
        return Err(format!(
            "rewrite {i} has a file that is not a path: {}",
            repr_str(bad)
        ));
    }
    Ok(Rewrite {
        title,
        description,
        files,
        acceptance_criteria: criteria,
        size,
    })
}

/// A `decision` is prose, or an object with a question and options; `null` is no decision.
fn decision_of(value: &Value) -> Result<Option<Decision>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(prose) => Ok(Some(Decision::Prose(prose.clone()))),
        Value::Object(object) => {
            let question = text_field(object.get("question"))
                .ok_or("decision is missing a question")?
                .to_owned();
            let raw_options = match object.get("options") {
                None => Vec::new(),
                Some(Value::Array(items)) => items.clone(),
                Some(_) => return Err("decision options must be a list".to_owned()),
            };
            let options: Vec<Value> = raw_options.into_iter().map(option_spec).collect();
            if let Some(problem) = decision_request_problem(&question, &options) {
                return Err(format!("decision options are not valid: {problem}"));
            }
            Ok(Some(Decision::Structured { question, options }))
        }
        _ => Err("decision must be a string or an object with question and options".to_owned()),
    }
}

/// A bare option label becomes `{"label": ...}`; an option object is kept as written.
fn option_spec(option: Value) -> Value {
    match option {
        Value::String(label) => {
            let mut object = Map::new();
            object.insert("label".to_owned(), Value::String(label));
            Value::Object(object)
        }
        other => other,
    }
}

/// Why a decision request is invalid, checked against `DecisionRequest` and `DecisionOptionIn`
/// (which strip whitespace from every string before measuring it). `None` when valid.
fn decision_request_problem(question: &str, options: &[Value]) -> Option<String> {
    if question.trim().chars().count() > QUESTION_MAX_CHARS {
        return Some("question is longer than 5000 characters".to_owned());
    }
    if options.len() > OPTIONS_MAX {
        return Some("at most 26 options are allowed".to_owned());
    }
    options.iter().enumerate().find_map(|(i, option)| {
        option_problem(option).map(|problem| format!("option {i}: {problem}"))
    })
}

fn option_problem(option: &Value) -> Option<String> {
    let Some(object) = option.as_object() else {
        return Some("it must be an object or a label".to_owned());
    };
    if let Some(extra) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "id" | "label" | "detail" | "recommended"))
    {
        return Some(format!("extra field {extra} is not permitted"));
    }
    match object.get("label") {
        Some(Value::String(label)) => {
            let length = label.trim().chars().count();
            if length == 0 || length > OPTION_LABEL_MAX_CHARS {
                return Some("label must be 1 to 500 characters".to_owned());
            }
        }
        _ => return Some("label is required".to_owned()),
    }
    match object.get("id") {
        None | Some(Value::Null) => {}
        Some(Value::String(id)) => {
            let id = id.trim();
            let shape_ok = id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
            if !shape_ok || id.chars().count() > OPTION_ID_MAX_CHARS {
                return Some("id must be a short slug".to_owned());
            }
        }
        Some(_) => return Some("id must be a string".to_owned()),
    }
    match object.get("detail") {
        None => {}
        Some(Value::String(detail)) => {
            if detail.trim().chars().count() > OPTION_DETAIL_MAX_CHARS {
                return Some("detail is longer than 5000 characters".to_owned());
            }
        }
        Some(_) => return Some("detail must be a string".to_owned()),
    }
    match object.get("recommended") {
        None | Some(Value::Bool(_)) => None,
        Some(_) => Some("recommended must be a boolean".to_owned()),
    }
}

fn build_steps(entry: &Entry, target: &TriageTarget, source: Option<&str>) -> Vec<Step> {
    let mut steps = Vec::new();
    // A pending verdict being replaced: drop its tags so the task carries only the new one. The
    // plain `needs-decision` tag goes too, but only when triage set it.
    let mut stale: Vec<String> = target
        .tags
        .iter()
        .filter(|tag| tag.starts_with(TRIAGE_TAG_PREFIX))
        .cloned()
        .collect();
    if stale.iter().any(|tag| tag == "triage:needs-decision")
        && target.tags.iter().any(|tag| tag == "needs-decision")
    {
        stale.push("needs-decision".to_owned());
    }
    if !stale.is_empty() {
        steps.push(Step::RemoveTags(stale));
    }

    let summary = entry.summary.as_str();
    let evidence = entry.evidence.as_slice();
    let verdict = entry.verdict.as_str();
    match entry.verdict {
        Verdict::Resolved | Verdict::Obsolete => {
            steps.push(Step::Comment(comment_body(
                source,
                verdict,
                summary,
                evidence,
                &[],
            )));
            steps.push(Step::AddTags(vec![format!("{TRIAGE_TAG_PREFIX}{verdict}")]));
            steps.push(Step::Cancel(format!("triage: {verdict}")));
        }
        Verdict::Open => {
            for rewrite in &entry.rewrites {
                steps.push(Step::CreateFollowUp(follow_up(entry, target, rewrite)));
            }
            for duplicate in &entry.duplicates {
                steps.push(Step::MarkDuplicate {
                    target: duplicate.clone(),
                });
            }
            let superseded = Superseded {
                source: source.map(str::to_owned),
                summary: summary.to_owned(),
                evidence: evidence.to_vec(),
                duplicates: entry.duplicates.clone(),
            };
            steps.push(Step::CommentSuperseded(superseded.clone()));
            steps.push(Step::AddTags(vec![format!(
                "{TRIAGE_TAG_PREFIX}superseded"
            )]));
            steps.push(Step::CancelSuperseded(superseded));
        }
        Verdict::NeedsDecision => {
            let question = match &entry.decision {
                Some(Decision::Prose(text)) => Some(text.as_str()),
                Some(Decision::Structured { question, .. }) => Some(question.as_str()),
                None => None,
            };
            let blocks: Vec<String> = question
                .filter(|question| !question.is_empty())
                .map(|question| format!("Decision: {question}"))
                .into_iter()
                .collect();
            steps.push(Step::Comment(comment_body(
                source,
                "needs a decision",
                summary,
                evidence,
                &blocks,
            )));
            if let Some(Decision::Structured { question, options }) = &entry.decision {
                steps.push(Step::RequestDecision {
                    question: question.clone(),
                    options: options.clone(),
                });
            }
            steps.push(Step::AddTags(vec![
                "needs-decision".to_owned(),
                format!("{TRIAGE_TAG_PREFIX}needs-decision"),
            ]));
        }
        Verdict::NeedsRun => {
            let command = entry.command.as_deref().unwrap_or_default();
            let blocks = [format!("Confirm with: {command}")];
            steps.push(Step::Comment(comment_body(
                source,
                "needs a run to confirm",
                summary,
                evidence,
                &blocks,
            )));
            steps.push(Step::AddTags(vec![format!("{TRIAGE_TAG_PREFIX}needs-run")]));
        }
        Verdict::Unclear => {
            steps.push(Step::Comment(comment_body(
                source,
                "unclear",
                summary,
                evidence,
                &[],
            )));
            steps.push(Step::AddTags(vec![format!("{TRIAGE_TAG_PREFIX}unclear")]));
        }
    }
    steps
}

/// Builds one follow-up from a rewrite: the original's priority, projects and repositories; its
/// tags minus the dropped and `kind:` ones, plus `triaged` and the scope.
fn follow_up(entry: &Entry, target: &TriageTarget, rewrite: &Rewrite) -> FollowUp {
    let mut description = rewrite.description.clone();
    if !rewrite.files.is_empty() {
        description.push_str(&format!("\n\n**Files:** {}", rewrite.files.join(", ")));
    }
    description.push_str(&format!(
        "\n\nTriaged from {}: {}",
        target.key, entry.summary
    ));

    let mut tags: Vec<String> = target
        .tags
        .iter()
        .filter(|tag| tag.as_str() != DROPPED_TAG && !tag.starts_with(KIND_TAG_PREFIX))
        .cloned()
        .collect();
    tags.push("triaged".to_owned());
    if let Some(size) = &rewrite.size {
        tags.push(format!("scope:{}", legacy_size(size)));
    }
    let mut seen = BTreeSet::new();
    tags.retain(|tag| seen.insert(tag.clone()));

    FollowUp {
        title: rewrite.title.clone(),
        description,
        acceptance_criteria: rewrite.acceptance_criteria.clone(),
        priority: target.priority,
        tags,
        files: rewrite
            .files
            .iter()
            .filter(|spec| !spec.trim().is_empty())
            .map(|spec| parse_file_spec(spec))
            .collect(),
        projects: target.projects.clone(),
        repositories: target.repositories.clone(),
        follows_up: target.key.clone(),
    }
}

/// The lead line, then any blocks, then the evidence list.
pub fn comment_body(
    source: Option<&str>,
    lead: &str,
    summary: &str,
    evidence: &[String],
    blocks: &[String],
) -> String {
    let head = match source {
        Some(source) if !source.is_empty() => format!("Triage ({source}): "),
        _ => "Triage: ".to_owned(),
    };
    let mut text = format!("{head}{lead}. {summary}").trim_end().to_owned();
    for block in blocks {
        text.push_str("\n\n");
        text.push_str(block);
    }
    if !evidence.is_empty() {
        text.push_str("\n\nEvidence:\n");
        let lines: Vec<String> = evidence.iter().map(|line| format!("- {line}")).collect();
        text.push_str(&lines.join("\n"));
    }
    text
}

/// A rewrite size mapped to the scope vocabulary; legacy sizes keep the scope they always meant.
pub fn legacy_size(size: &str) -> &str {
    match size {
        "small" => "one-file",
        "medium" => "one-crate",
        "large" => "several-crates",
        other => other,
    }
}

/// Splits a file spec `PATH[:ANCHOR]`. A Windows drive letter belongs to the path; the first
/// colon after it starts the anchor. Path and anchor are trimmed, and an empty anchor is `None`.
/// Mirrors `parse_task_file`; [`is_file_spec`] says whether the result is a valid `TaskFileIn`.
pub fn parse_file_spec(spec: &str) -> FileSpec {
    let spec = spec.trim();
    let bytes = spec.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let body_start = if drive { 2 } else { 0 };
    let (path, anchor) = match spec[body_start..].find(':') {
        Some(colon) => (
            &spec[..body_start + colon],
            Some(&spec[body_start + colon + 1..]),
        ),
        None => (spec, None),
    };
    FileSpec {
        path: path.trim().to_owned(),
        anchor: anchor
            .map(str::trim)
            .filter(|anchor| !anchor.is_empty())
            .map(str::to_owned),
    }
}

/// Whether a non-blank spec parses to a valid `TaskFileIn`: a path of 1 to 1000 characters and
/// an anchor of at most 300.
pub fn is_file_spec(spec: &str) -> bool {
    let parsed = parse_file_spec(spec);
    let path_len = parsed.path.chars().count();
    let anchor_len = parsed.anchor.as_deref().map_or(0, |a| a.chars().count());
    (1..=FILE_PATH_MAX_CHARS).contains(&path_len) && anchor_len <= FILE_ANCHOR_MAX_CHARS
}

/// A string field that is non-blank after trimming.
fn text_field(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.as_str()),
        _ => None,
    }
}

/// An array of strings, or `None` when it is not one.
fn str_list(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// A value as Python's `repr` would show it for the shapes a triage file holds.
fn repr(value: &Value) -> String {
    match value {
        Value::String(text) => repr_str(text),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        other => other.to_string(),
    }
}

fn repr_str(text: &str) -> String {
    format!("'{text}'")
}
