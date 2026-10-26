//! Rename a term across a project's live records (CORE-05). Mirrors `core/services/rename.py`:
//! the word matcher ([`Term`]) and the plan of which project, repository, verification config and
//! task records change.
//!
//! The plan is data. The shell reads the records, applies the plan through the services (one
//! event per record), and reports a `path_rewrites` candidate only when the path exists before and
//! not after, since that check touches the disk.

use serde_json::{Map, Value};
use wg_types::{ErrorCode, Id, TaskStatus, WgError};

/// Closed tasks keep their text: accepted and cancelled work, and anything archived.
fn is_closed(status: TaskStatus, archived: bool) -> bool {
    archived || matches!(status, TaskStatus::Accepted | TaskStatus::Cancelled)
}

/// The old and new spelling of one word, and the tokens that contain it but are not it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    old: String,
    new: String,
    keep: Vec<String>,
}

/// What may follow a match: the lowercase and title spellings need no lowercase letter after them,
/// the uppercase spelling no letter at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    NotLower,
    NotLetter,
}

impl Term {
    /// Validates the two names: one ASCII word of letters each, and different ignoring case.
    pub fn new(old: &str, new: &str, keep: &[&str]) -> Result<Self, WgError> {
        for word in [old, new] {
            if !is_name(word) {
                return Err(WgError::new(
                    ErrorCode::ValidationFailed,
                    format!("'{word}' is not a name: one ASCII word, letters only"),
                ));
            }
        }
        if old.eq_ignore_ascii_case(new) {
            return Err(WgError::new(
                ErrorCode::ValidationFailed,
                "the old and new names are the same word",
            ));
        }
        Ok(Self {
            old: old.to_owned(),
            new: new.to_owned(),
            keep: keep.iter().map(|token| (*token).to_owned()).collect(),
        })
    }

    /// The old word as written in the record.
    pub fn old(&self) -> &str {
        &self.old
    }

    /// The new word as written in the record.
    pub fn new_word(&self) -> &str {
        &self.new
    }

    /// `text` with the term renamed, and how many sites changed.
    pub fn apply(&self, text: &str) -> (String, usize) {
        let masked = self
            .keep
            .iter()
            .enumerate()
            .fold(text.to_owned(), |masked, (i, token)| {
                masked.replace(token.as_str(), &mask(i))
            });
        let (masked, total) =
            self.spellings()
                .iter()
                .fold((masked, 0), |(masked, total), (old, new, after)| {
                    let (next, count) = substitute(&masked, old, new, *after);
                    (next, total + count)
                });
        let restored = self
            .keep
            .iter()
            .enumerate()
            .fold(masked, |masked, (i, token)| masked.replace(&mask(i), token));
        (restored, total)
    }

    /// Renames inside a JSON value: every string, and every object key.
    pub fn deep(&self, value: &Value) -> (Value, usize) {
        match value {
            Value::String(text) => {
                let (out, sites) = self.apply(text);
                (Value::String(out), sites)
            }
            Value::Array(items) => {
                let parts: Vec<(Value, usize)> = items.iter().map(|item| self.deep(item)).collect();
                let total = parts.iter().map(|(_, sites)| sites).sum();
                (
                    Value::Array(parts.into_iter().map(|(item, _)| item).collect()),
                    total,
                )
            }
            Value::Object(map) => {
                let parts: Vec<(String, Value, usize)> = map
                    .iter()
                    .map(|(key, item)| {
                        let (new_key, key_sites) = self.apply(key);
                        let (new_item, item_sites) = self.deep(item);
                        (new_key, new_item, key_sites + item_sites)
                    })
                    .collect();
                let total = parts.iter().map(|(_, _, sites)| sites).sum();
                (
                    Value::Object(
                        parts
                            .into_iter()
                            .map(|(key, item, _)| (key, item))
                            .collect::<Map<String, Value>>(),
                    ),
                    total,
                )
            }
            other => (other.clone(), 0),
        }
    }

    /// The three spellings, applied in the order the Python patterns are: lowercase, title, upper.
    fn spellings(&self) -> [(String, String, After); 3] {
        [
            (
                self.old.to_ascii_lowercase(),
                self.new.to_ascii_lowercase(),
                After::NotLower,
            ),
            (title(&self.old), title(&self.new), After::NotLower),
            (
                self.old.to_ascii_uppercase(),
                self.new.to_ascii_uppercase(),
                After::NotLetter,
            ),
        ]
    }
}

/// The placeholder that stands in for the kept token at `index` while the word is matched.
fn mask(index: usize) -> String {
    format!("\u{0}{index}\u{0}")
}

fn is_name(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|b| b.is_ascii_alphabetic())
}

/// Python's `word[:1].upper() + word[1:].lower()`; the words are ASCII.
fn title(word: &str) -> String {
    let (first, rest) = word.split_at(word.len().min(1));
    format!(
        "{}{}",
        first.to_ascii_uppercase(),
        rest.to_ascii_lowercase()
    )
}

/// Replaces every non-overlapping, boundary-respecting match of `old` with `new`, scanning left to
/// right like `re.subn`. Lookbehind and lookahead are written out by hand: the `regex` crate has
/// no lookaround.
fn substitute(text: &str, old: &str, new: &str, after: After) -> (String, usize) {
    // Every offset where `old` starts is a candidate; `old` starts with an ASCII letter, so each
    // offset is a char boundary. A rejected candidate resumes the scan one byte on, an accepted
    // one at its end, which is what `pos` tracks.
    let (out, emitted, _, count) = (0..text.len())
        .filter(|&start| text.as_bytes()[start..].starts_with(old.as_bytes()))
        .fold(
            (String::with_capacity(text.len()), 0, 0, 0),
            |(out, emitted, pos, count), start| {
                let end = start + old.len();
                if start >= pos && before_ok(text, start) && after_ok(text, end, after) {
                    let out = format!("{out}{}{new}", &text[emitted..start]);
                    (out, end, end, count + 1)
                } else {
                    (out, emitted, pos, count)
                }
            },
        );
    (format!("{out}{}", &text[emitted..]), count)
}

/// Not preceded by an ASCII letter, or preceded by the escape `\n` or `\t` (a boundary).
fn before_ok(text: &str, start: usize) -> bool {
    match text[..start].chars().next_back() {
        None => true,
        Some(c) if !c.is_ascii_alphabetic() => true,
        Some(_) => {
            let bytes = text.as_bytes();
            start >= 2 && bytes[start - 2] == b'\\' && matches!(bytes[start - 1], b'n' | b't')
        }
    }
}

fn after_ok(text: &str, end: usize, after: After) -> bool {
    match text[end..].chars().next() {
        None => true,
        Some(c) => match after {
            After::NotLower => !c.is_ascii_lowercase(),
            After::NotLetter => !c.is_ascii_alphabetic(),
        },
    }
}

/// The project fields the rename reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: Id,
    pub slug: String,
    pub name: String,
    pub description: String,
}

/// The repository fields the rename reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRecord {
    pub id: Id,
    pub name: String,
    pub description: String,
}

/// A verification config: `checks` is the JSON list of check objects.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigRecord {
    pub name: String,
    pub scope_type: String,
    pub scope_id: Id,
    pub checks: Value,
}

/// A file row of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub anchor: Option<String>,
    pub note: String,
}

/// The task fields the rename reads. `tags` keep the order the shell reads them in.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRecord {
    pub id: Id,
    pub key: String,
    pub status: TaskStatus,
    pub archived: bool,
    pub version: i64,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub files: Vec<FileRecord>,
    pub tags: Vec<String>,
}

/// A project rename. Each `Some` is the new value of a field that changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChange {
    pub id: Id,
    pub old_slug: String,
    pub new_slug: String,
    pub old_name: String,
    pub new_name: String,
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// A repository rename. The checkouts on disk are not renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryChange {
    pub id: Id,
    pub old_name: String,
    pub new_name: String,
    pub description_sites: usize,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// A verification config whose checks change.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigChange {
    pub name: String,
    /// The scope the config is saved back to: `project` or `repository`, and the record's id.
    pub scope_type: String,
    pub scope_id: Id,
    pub sites: usize,
    pub checks: Value,
    /// Absolute paths whose spelling changed, as `(before, after)`. The shell keeps the pairs where
    /// `before` exists and `after` does not: a check naming a path nobody renamed will fail.
    pub path_rewrites: Vec<(String, String)>,
}

/// A task whose text or tags change.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskChange {
    pub id: Id,
    pub key: String,
    pub status: TaskStatus,
    pub closed: bool,
    pub version: i64,
    /// The changed counts per field, in the order the report lists them.
    pub fields: Vec<(&'static str, usize)>,
    pub tags: Option<Vec<String>>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub files: Option<Vec<FileRecord>>,
}

/// The rename plan and its report counters.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenamePlan {
    pub project: Option<ProjectChange>,
    pub repositories: Vec<RepositoryChange>,
    pub verification_configs: Vec<ConfigChange>,
    pub tasks: Vec<TaskChange>,
    pub tags_renamed: usize,
    /// Closed tasks that mention the term and keep their text.
    pub closed_kept: usize,
}

/// Plans the rename of `term` across one project. Repositories come first, then the project,
/// then the verification configs scoped to the project or its repositories, then the tasks, in
/// the order the caller gave them.
pub fn plan_rename(
    term: &Term,
    project: &ProjectRecord,
    repositories: &[RepositoryRecord],
    configs: &[ConfigRecord],
    tasks: &[TaskRecord],
) -> RenamePlan {
    let repository_changes: Vec<RepositoryChange> = repositories
        .iter()
        .filter_map(|repo| {
            let (name, name_sites) = term.apply(&repo.name);
            let (description, description_sites) = term.apply(&repo.description);
            (name_sites + description_sites > 0).then(|| RepositoryChange {
                id: repo.id,
                old_name: repo.name.clone(),
                new_name: name.clone(),
                description_sites,
                name: (name_sites > 0).then_some(name),
                description: (description_sites > 0).then_some(description),
            })
        })
        .collect();

    let (slug, slug_sites) = term.apply(&project.slug);
    let (name, name_sites) = term.apply(&project.name);
    let (description, description_sites) = term.apply(&project.description);
    let project_change = (slug_sites + name_sites + description_sites > 0).then(|| ProjectChange {
        id: project.id,
        old_slug: project.slug.clone(),
        new_slug: slug.clone(),
        old_name: project.name.clone(),
        new_name: name.clone(),
        slug: (slug_sites > 0).then_some(slug),
        name: (name_sites > 0).then_some(name),
        description: (description_sites > 0).then_some(description),
    });

    let in_scope = |config: &ConfigRecord| {
        (config.scope_type == "project" && config.scope_id == project.id)
            || (config.scope_type == "repository"
                && repositories.iter().any(|repo| repo.id == config.scope_id))
    };
    let config_changes: Vec<ConfigChange> = configs
        .iter()
        .filter(|config| in_scope(config))
        .filter_map(|config| {
            let (checks, sites) = term.deep(&config.checks);
            (sites > 0).then(|| ConfigChange {
                name: config.name.clone(),
                scope_type: config.scope_type.clone(),
                scope_id: config.scope_id,
                sites,
                path_rewrites: path_rewrites(&config.checks, &checks),
                checks,
            })
        })
        .collect();

    let outcomes: Vec<TaskOutcome> = tasks.iter().map(|task| plan_task(term, task)).collect();
    RenamePlan {
        project: project_change,
        repositories: repository_changes,
        verification_configs: config_changes,
        tags_renamed: outcomes.iter().map(|outcome| outcome.tags_renamed).sum(),
        closed_kept: outcomes
            .iter()
            .filter(|outcome| outcome.closed_kept)
            .count(),
        tasks: outcomes
            .into_iter()
            .filter_map(|outcome| outcome.change)
            .collect(),
    }
}

/// What planning one task yields: its change when something changes, the tag sites counted for
/// the report, and whether it is a closed task that mentions the term.
struct TaskOutcome {
    change: Option<TaskChange>,
    tags_renamed: usize,
    closed_kept: bool,
}

fn plan_task(term: &Term, task: &TaskRecord) -> TaskOutcome {
    let closed = is_closed(task.status, task.archived);
    let renamed_tags: Vec<String> = task.tags.iter().map(|tag| term.apply(tag).0).collect();
    let tag_sites = renamed_tags
        .iter()
        .zip(&task.tags)
        .filter(|(new, old)| new != old)
        .count();
    let tags = (tag_sites > 0).then_some(renamed_tags);

    // Closed records keep their words; a mention is counted, the text is left alone.
    let closed_kept = closed && text_sites(term, task) > 0;

    let title = (!closed)
        .then(|| term.apply(&task.title))
        .filter(|(_, n)| *n > 0);
    let description = (!closed)
        .then(|| term.apply(&task.description))
        .filter(|(_, n)| *n > 0);
    let criteria = (!closed)
        .then(|| {
            let criteria: Vec<(String, usize)> = task
                .acceptance_criteria
                .iter()
                .map(|c| term.apply(c))
                .collect();
            let sites: usize = criteria.iter().map(|(_, n)| n).sum();
            (
                criteria.into_iter().map(|(c, _)| c).collect::<Vec<_>>(),
                sites,
            )
        })
        .filter(|(_, n)| *n > 0);
    let files = (!closed)
        .then(|| {
            let files: Vec<(FileRecord, usize)> = task
                .files
                .iter()
                .map(|file| {
                    let (path, path_sites) = term.apply(&file.path);
                    let (anchor, anchor_sites) = term.apply(file.anchor.as_deref().unwrap_or(""));
                    let record = FileRecord {
                        path,
                        anchor: (!anchor.is_empty()).then_some(anchor),
                        note: file.note.clone(),
                    };
                    (record, path_sites + anchor_sites)
                })
                .collect();
            let sites: usize = files.iter().map(|(_, n)| n).sum();
            (files.into_iter().map(|(f, _)| f).collect::<Vec<_>>(), sites)
        })
        .filter(|(_, n)| *n > 0);

    let fields: Vec<(&'static str, usize)> = [
        ("tags", (tag_sites > 0).then_some(tag_sites)),
        ("title", title.as_ref().map(|(_, n)| *n)),
        ("description", description.as_ref().map(|(_, n)| *n)),
        ("acceptance_criteria", criteria.as_ref().map(|(_, n)| *n)),
        ("files", files.as_ref().map(|(_, n)| *n)),
    ]
    .into_iter()
    .filter_map(|(name, sites)| sites.map(|n| (name, n)))
    .collect();

    if fields.is_empty() {
        return TaskOutcome {
            change: None,
            tags_renamed: 0,
            closed_kept,
        };
    }
    TaskOutcome {
        change: Some(TaskChange {
            id: task.id,
            key: task.key.clone(),
            status: task.status,
            closed,
            version: task.version,
            fields,
            tags,
            title: title.map(|(text, _)| text),
            description: description.map(|(text, _)| text),
            acceptance_criteria: criteria.map(|(items, _)| items),
            files: files.map(|(items, _)| items),
        }),
        tags_renamed: tag_sites,
        closed_kept,
    }
}

/// Every text a closed task carries that a mention of the term could sit in.
fn text_sites(term: &Term, task: &TaskRecord) -> usize {
    [task.title.as_str(), task.description.as_str()]
        .into_iter()
        .chain(task.acceptance_criteria.iter().map(String::as_str))
        .chain(
            task.files
                .iter()
                .flat_map(|file| [file.path.as_str(), file.anchor.as_deref().unwrap_or("")]),
        )
        .map(|text| term.apply(text).1)
        .sum()
}

/// Candidate `(before, after)` absolute-path rewrites in a structural diff of a config's checks.
/// Strings are compared where they sit; lists zip by position, objects by their values in order.
pub fn path_rewrites(before: &Value, after: &Value) -> Vec<(String, String)> {
    match (before, after) {
        (Value::String(b), Value::String(a)) => {
            if b != a && is_absolute(a) {
                vec![(b.clone(), a.clone())]
            } else {
                Vec::new()
            }
        }
        (Value::Array(b), Value::Array(a)) => b
            .iter()
            .zip(a)
            .flat_map(|(b, a)| path_rewrites(b, a))
            .collect(),
        (Value::Object(b), Value::Object(a)) => b
            .values()
            .zip(a.values())
            .flat_map(|(b, a)| path_rewrites(b, a))
            .collect(),
        _ => Vec::new(),
    }
}

/// A POSIX or Windows absolute spelling: `/x`, `\\x`, or a drive letter followed by a separator.
fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    match bytes {
        [b'/' | b'\\', ..] => true,
        [drive, b':', b'/' | b'\\', ..] => drive.is_ascii_alphabetic(),
        _ => false,
    }
}
