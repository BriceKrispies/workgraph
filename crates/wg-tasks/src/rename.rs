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
        let mut masked = text.to_owned();
        for (i, token) in self.keep.iter().enumerate() {
            masked = masked.replace(token.as_str(), &mask(i));
        }
        let mut total = 0;
        for (old, new, after) in self.spellings() {
            let (next, count) = substitute(&masked, &old, &new, after);
            masked = next;
            total += count;
        }
        for (i, token) in self.keep.iter().enumerate() {
            masked = masked.replace(&mask(i), token);
        }
        (masked, total)
    }

    /// Renames inside a JSON value: every string, and every object key.
    pub fn deep(&self, value: &Value) -> (Value, usize) {
        match value {
            Value::String(text) => {
                let (out, sites) = self.apply(text);
                (Value::String(out), sites)
            }
            Value::Array(items) => {
                let mut total = 0;
                let out = items
                    .iter()
                    .map(|item| {
                        let (renamed, sites) = self.deep(item);
                        total += sites;
                        renamed
                    })
                    .collect();
                (Value::Array(out), total)
            }
            Value::Object(map) => {
                let mut out = Map::new();
                let mut total = 0;
                for (key, item) in map {
                    let (new_key, key_sites) = self.apply(key);
                    let (new_item, item_sites) = self.deep(item);
                    out.insert(new_key, new_item);
                    total += key_sites + item_sites;
                }
                (Value::Object(out), total)
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
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = first.to_ascii_uppercase().to_string();
            out.push_str(&chars.as_str().to_ascii_lowercase());
            out
        }
    }
}

/// Replaces every non-overlapping, boundary-respecting match of `old` with `new`, scanning left to
/// right like `re.subn`. Lookbehind and lookahead are written out by hand: the `regex` crate has
/// no lookaround.
fn substitute(text: &str, old: &str, new: &str, after: After) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut emitted = 0;
    let mut pos = 0;
    let mut count = 0;
    while let Some(found) = text[pos..].find(old) {
        let start = pos + found;
        let end = start + old.len();
        if before_ok(text, start) && after_ok(text, end, after) {
            out.push_str(&text[emitted..start]);
            out.push_str(new);
            emitted = end;
            pos = end;
            count += 1;
        } else {
            // `old` starts with an ASCII letter, so the next byte is a char boundary.
            pos = start + 1;
        }
    }
    out.push_str(&text[emitted..]);
    (out, count)
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
    let mut plan = RenamePlan::default();

    for repo in repositories {
        let (name, name_sites) = term.apply(&repo.name);
        let (description, description_sites) = term.apply(&repo.description);
        if name_sites + description_sites == 0 {
            continue;
        }
        plan.repositories.push(RepositoryChange {
            id: repo.id,
            old_name: repo.name.clone(),
            new_name: name.clone(),
            description_sites,
            name: (name_sites > 0).then_some(name),
            description: (description_sites > 0).then_some(description),
        });
    }

    let (slug, slug_sites) = term.apply(&project.slug);
    let (name, name_sites) = term.apply(&project.name);
    let (description, description_sites) = term.apply(&project.description);
    if slug_sites + name_sites + description_sites > 0 {
        plan.project = Some(ProjectChange {
            id: project.id,
            old_slug: project.slug.clone(),
            new_slug: slug.clone(),
            old_name: project.name.clone(),
            new_name: name.clone(),
            slug: (slug_sites > 0).then_some(slug),
            name: (name_sites > 0).then_some(name),
            description: (description_sites > 0).then_some(description),
        });
    }

    let in_scope = |config: &ConfigRecord| {
        (config.scope_type == "project" && config.scope_id == project.id)
            || (config.scope_type == "repository"
                && repositories.iter().any(|repo| repo.id == config.scope_id))
    };
    for config in configs.iter().filter(|config| in_scope(config)) {
        let (checks, sites) = term.deep(&config.checks);
        if sites == 0 {
            continue;
        }
        let path_rewrites = path_rewrites(&config.checks, &checks);
        plan.verification_configs.push(ConfigChange {
            name: config.name.clone(),
            sites,
            checks,
            path_rewrites,
        });
    }

    for task in tasks {
        plan_task(term, task, &mut plan);
    }
    plan
}

fn plan_task(term: &Term, task: &TaskRecord, plan: &mut RenamePlan) {
    let closed = is_closed(task.status, task.archived);
    let renamed_tags: Vec<String> = task.tags.iter().map(|tag| term.apply(tag).0).collect();
    let tag_sites = renamed_tags
        .iter()
        .zip(&task.tags)
        .filter(|(new, old)| new != old)
        .count();

    let mut change = TaskChange {
        id: task.id,
        key: task.key.clone(),
        status: task.status,
        closed,
        version: task.version,
        fields: Vec::new(),
        tags: None,
        title: None,
        description: None,
        acceptance_criteria: None,
        files: None,
    };
    if tag_sites > 0 {
        change.tags = Some(renamed_tags);
        change.fields.push(("tags", tag_sites));
    }

    if closed {
        // Closed records keep their words; a mention is counted, the text is left alone.
        if text_sites(term, task) > 0 {
            plan.closed_kept += 1;
        }
    } else {
        let (title, n) = term.apply(&task.title);
        if n > 0 {
            change.title = Some(title);
            change.fields.push(("title", n));
        }
        let (description, n) = term.apply(&task.description);
        if n > 0 {
            change.description = Some(description);
            change.fields.push(("description", n));
        }
        let criteria: Vec<(String, usize)> = task
            .acceptance_criteria
            .iter()
            .map(|c| term.apply(c))
            .collect();
        let criteria_sites: usize = criteria.iter().map(|(_, n)| n).sum();
        if criteria_sites > 0 {
            change.acceptance_criteria = Some(criteria.into_iter().map(|(c, _)| c).collect());
            change.fields.push(("acceptance_criteria", criteria_sites));
        }
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
        let file_sites: usize = files.iter().map(|(_, n)| n).sum();
        if file_sites > 0 {
            change.files = Some(files.into_iter().map(|(f, _)| f).collect());
            change.fields.push(("files", file_sites));
        }
    }

    if change.fields.is_empty() {
        return;
    }
    plan.tags_renamed += tag_sites;
    plan.tasks.push(change);
}

/// Every text a closed task carries that a mention of the term could sit in.
fn text_sites(term: &Term, task: &TaskRecord) -> usize {
    let mut texts: Vec<&str> = vec![task.title.as_str(), task.description.as_str()];
    texts.extend(task.acceptance_criteria.iter().map(String::as_str));
    for file in &task.files {
        texts.push(file.path.as_str());
        texts.push(file.anchor.as_deref().unwrap_or(""));
    }
    texts.iter().map(|text| term.apply(text).1).sum()
}

/// Candidate `(before, after)` absolute-path rewrites in a structural diff of a config's checks.
/// Strings are compared where they sit; lists zip by position, objects by their values in order.
pub fn path_rewrites(before: &Value, after: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_path_rewrites(before, after, &mut out);
    out
}

fn collect_path_rewrites(before: &Value, after: &Value, out: &mut Vec<(String, String)>) {
    match (before, after) {
        (Value::String(b), Value::String(a)) => {
            if b != a && is_absolute(a) {
                out.push((b.clone(), a.clone()));
            }
        }
        (Value::Array(b), Value::Array(a)) => {
            for (b, a) in b.iter().zip(a) {
                collect_path_rewrites(b, a, out);
            }
        }
        (Value::Object(b), Value::Object(a)) => {
            for (b, a) in b.values().zip(a.values()) {
                collect_path_rewrites(b, a, out);
            }
        }
        _ => {}
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
