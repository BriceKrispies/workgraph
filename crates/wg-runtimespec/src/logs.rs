//! JSON log lines (contract O14) and the rotation rule of the log file (F11), as
//! `core/logging.py` produces them.
//!
//! A line is `json.dumps(payload, default=str)` of `{"ts", "level", "logger", "msg", <extra
//! fields>, "exc"?}`. Records go to stderr and to `<logs>/workgraph.log`, a `RotatingFileHandler`
//! of 5,000,000 bytes with 5 backups. Only the `workgraph` logger tree is configured; the shell
//! drops records of other targets, as Python leaves other loggers alone.

use std::path::{Path, PathBuf};

use wg_json::{DumpError, PyStr, Value, log_json};
use wg_types::{Timestamp, WgError};
use wg_wire::convention::text;

/// The log file in `<logs>`.
pub const LOG_FILE_NAME: &str = "workgraph.log";
/// `RotatingFileHandler(maxBytes=5_000_000)`.
pub const MAX_BYTES: u64 = 5_000_000;
/// `RotatingFileHandler(backupCount=5)`.
pub const BACKUP_COUNT: u32 = 5;
/// The root of the configured logger tree.
pub const ROOT_LOGGER: &str = "workgraph";

/// The attributes of a `logging.LogRecord` (plus `message` and `asctime`): an extra field with
/// one of these names never reaches the payload.
pub const RESERVED_FIELDS: &[&str] = &[
    "name",
    "msg",
    "args",
    "levelname",
    "levelno",
    "pathname",
    "filename",
    "module",
    "exc_info",
    "exc_text",
    "stack_info",
    "lineno",
    "funcName",
    "created",
    "msecs",
    "relativeCreated",
    "thread",
    "threadName",
    "processName",
    "process",
    "taskName",
    "message",
    "asctime",
];

/// A Python logging level, ordered by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// `NOTSET`: as a threshold, everything passes.
    NotSet,
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl Level {
    /// `logger.setLevel(level.upper())`: the level names Python accepts (`WARN` and `FATAL`
    /// included). Anything else is the `ValueError` Python raises, as a `config_error`.
    pub fn parse(text: &str) -> Result<Level, WgError> {
        match text.to_uppercase().as_str() {
            "NOTSET" => Ok(Level::NotSet),
            "DEBUG" => Ok(Level::Debug),
            "INFO" => Ok(Level::Info),
            "WARNING" | "WARN" => Ok(Level::Warning),
            "ERROR" => Ok(Level::Error),
            "CRITICAL" | "FATAL" => Ok(Level::Critical),
            other => Err(WgError::config(format!(
                "Unknown level: {}",
                wg_model::py_repr_str(other)
            ))),
        }
    }

    /// `record.levelname.lower()`.
    pub fn name(self) -> &'static str {
        match self {
            Level::NotSet => "notset",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Critical => "critical",
        }
    }

    /// The level of a `tracing` event. `TRACE` has no Python counterpart and logs as `debug`.
    pub fn from_tracing(level: &tracing_core::Level) -> Level {
        match *level {
            tracing_core::Level::ERROR => Level::Error,
            tracing_core::Level::WARN => Level::Warning,
            tracing_core::Level::INFO => Level::Info,
            _ => Level::Debug,
        }
    }

    /// Whether a record at `self` passes a logger set to `threshold`.
    pub fn passes(self, threshold: Level) -> bool {
        self >= threshold
    }
}

/// `get_logger(name)`: names outside the tree are put under `workgraph.`.
pub fn logger_name(name: &str) -> String {
    if name.starts_with(ROOT_LOGGER) {
        name.to_owned()
    } else {
        format!("{ROOT_LOGGER}.{name}")
    }
}

/// Whether records of logger (or `tracing` target) `name` belong to the configured tree.
pub fn in_logger_tree(name: &str) -> bool {
    name == ROOT_LOGGER
        || name
            .strip_prefix(ROOT_LOGGER)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// One record, as the formatter sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LogRecord {
    pub at: Timestamp,
    pub level: Level,
    pub logger: String,
    pub msg: String,
    /// `extra=` fields, in the order given.
    pub fields: Vec<(String, Value)>,
    /// The formatted exception, when the record carries one.
    pub exc: Option<String>,
}

/// `JsonFormatter.format`: the line without its newline.
pub fn format_record(record: &LogRecord) -> Result<String, DumpError> {
    let base = vec![
        ("ts".to_owned(), text(&record.at.format_isoformat())),
        ("level".to_owned(), text(record.level.name())),
        ("logger".to_owned(), text(&record.logger)),
        ("msg".to_owned(), text(&record.msg)),
    ];
    let extras = record
        .fields
        .iter()
        .filter(|(key, _)| !RESERVED_FIELDS.contains(&key.as_str()) && !key.starts_with('_'))
        .map(|(key, value)| (key.clone(), value.clone()));
    let exc = record.exc.iter().map(|exc| ("exc".to_owned(), text(exc)));
    let payload = extras.chain(exc).fold(base, upsert);
    log_json(&Value::Dict(
        payload
            .into_iter()
            .map(|(key, value)| (PyStr::from(key.as_str()), value))
            .collect(),
    ))
}

/// `payload[key] = value`: a key already present keeps its place.
fn upsert(payload: Vec<(String, Value)>, entry: (String, Value)) -> Vec<(String, Value)> {
    if payload.iter().any(|(key, _)| *key == entry.0) {
        payload
            .into_iter()
            .map(|(key, value)| {
                if key == entry.0 {
                    (key, entry.1.clone())
                } else {
                    (key, value)
                }
            })
            .collect()
    } else {
        payload.into_iter().chain([entry]).collect()
    }
}

/// The line terminator Python's text-mode streams write: a newline, translated to a carriage
/// return and a newline on Windows.
pub fn line_end(windows: bool) -> &'static str {
    if windows { "\r\n" } else { "\n" }
}

/// `RotatingFileHandler.shouldRollover`: the file holds `position` bytes and the next line
/// (newline included) is `line_chars` characters. An empty file never rolls over.
pub fn should_rollover(position: u64, line_chars: usize, max_bytes: u64) -> bool {
    max_bytes > 0
        && position > 0
        && position.saturating_add(u64::try_from(line_chars).unwrap_or(u64::MAX)) >= max_bytes
}

/// `RotatingFileHandler.doRollover`: the renames to perform, in order. Each source that exists
/// replaces its destination (removed first if present); a missing source is skipped.
pub fn rollover_moves(base: &Path, backup_count: u32) -> Vec<(PathBuf, PathBuf)> {
    if backup_count == 0 {
        return Vec::new();
    }
    let numbered = |index: u32| {
        let mut name = base.as_os_str().to_owned();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    };
    (1..backup_count)
        .rev()
        .map(|index| (numbered(index), numbered(index + 1)))
        .chain([(base.to_path_buf(), numbered(1))])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u32) -> Timestamp {
        Timestamp::from_components(2026, 10, 25, 12, 0, 5, micros).unwrap()
    }

    fn record(fields: Vec<(String, Value)>, exc: Option<&str>) -> LogRecord {
        LogRecord {
            at: at(123_456),
            level: Level::Info,
            logger: "workgraph.jobs.worker".to_owned(),
            msg: "claimed job".to_owned(),
            fields,
            exc: exc.map(str::to_owned),
        }
    }

    // covers: O14
    #[test]
    fn line_matches_the_python_formatter() {
        // json.dumps({"ts": ..., "level": "info", "logger": ..., "msg": ..., "job_id": "j1",
        // "attempt": 3, "ok": True, "name": "skipped", "_private": 1}, default=str)
        let line = format_record(&record(
            vec![
                ("job_id".to_owned(), text("j1")),
                ("attempt".to_owned(), Value::Int(3)),
                ("ok".to_owned(), Value::Bool(true)),
                ("name".to_owned(), text("skipped")),
                ("_private".to_owned(), Value::Int(1)),
            ],
            None,
        ))
        .unwrap();
        assert_eq!(
            line,
            r#"{"ts": "2026-10-25T12:00:05.123456+00:00", "level": "info", "logger": "workgraph.jobs.worker", "msg": "claimed job", "job_id": "j1", "attempt": 3, "ok": true}"#
        );
    }

    #[test]
    fn extras_overwrite_in_place_and_exc_comes_last() {
        let line = format_record(&LogRecord {
            at: at(0),
            msg: "caf\u{e9}".to_owned(),
            ..record(vec![("level".to_owned(), text("loud"))], Some("Traceback"))
        })
        .unwrap();
        assert_eq!(
            line,
            concat!(
                r#"{"ts": "2026-10-25T12:00:05+00:00", "level": "loud", "logger": "workgraph.jobs.worker", "msg": "caf"#,
                "\\u00e9",
                r#"", "exc": "Traceback"}"#
            )
        );
    }

    #[test]
    fn levels_and_names() {
        assert_eq!(Level::parse("warn").unwrap(), Level::Warning);
        assert_eq!(Level::parse("INFO").unwrap(), Level::Info);
        assert_eq!(
            Level::parse("loud").unwrap_err().message,
            "Unknown level: 'LOUD'"
        );
        assert!(Level::Warning.passes(Level::Info));
        assert!(!Level::Debug.passes(Level::Info));
        assert_eq!(logger_name("jobs"), "workgraph.jobs");
        assert_eq!(logger_name("workgraph.api"), "workgraph.api");
        assert!(in_logger_tree("workgraph") && in_logger_tree("workgraph.mcp"));
        assert!(!in_logger_tree("workgraphx") && !in_logger_tree("rusqlite"));
    }

    #[test]
    fn rotation() {
        assert!(!should_rollover(0, 10, 10));
        assert!(should_rollover(5, 5, 10));
        assert!(!should_rollover(4, 5, 10));
        assert!(!should_rollover(100, 5, 0));
        let moves = rollover_moves(Path::new("logs/workgraph.log"), 3);
        let names: Vec<(String, String)> = moves
            .iter()
            .map(|(a, b)| (a.display().to_string(), b.display().to_string()))
            .collect();
        assert_eq!(
            names,
            [
                ("logs/workgraph.log.2", "logs/workgraph.log.3"),
                ("logs/workgraph.log.1", "logs/workgraph.log.2"),
                ("logs/workgraph.log", "logs/workgraph.log.1"),
            ]
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
        );
        assert!(rollover_moves(Path::new("x"), 0).is_empty());
    }
}
