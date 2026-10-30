//! JSON logging (contract O14, file F11), as `core/logging.py` configures it.
//!
//! Code logs with `tracing` under a `workgraph.*` target (`tracing::info!(target:
//! "workgraph.jobs.worker", job_id = %id, "claimed job")`); the target is the Python logger name,
//! the message is `msg`, and the other fields are the `extra=` fields. [`JsonLayer`] writes each
//! record as one JSON line to stderr and to `<logs>/workgraph.log`, which rotates at 5,000,000
//! bytes keeping 5 backups. Records of targets outside the `workgraph` tree are dropped, as Python
//! leaves other loggers unconfigured.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::spec::logs::{
    BACKUP_COUNT, LOG_FILE_NAME, MAX_BYTES, in_logger_tree, line_end, rollover_moves,
    should_rollover,
};
use crate::spec::{Level, LogRecord, format_record, timestamp_from_unix_nanos};
use tracing_core::field::{Field, Visit};
use tracing_core::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::Registry;
use wg_json::{PyStr, Value};
use wg_types::WgError;

/// `configure_logging(level, log_dir, stderr=...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSettings {
    pub level: Level,
    /// Where `workgraph.log` goes; `None` logs to stderr only.
    pub log_dir: Option<PathBuf>,
    pub stderr: bool,
}

/// The installed sink, once `configure_logging` has run in this process.
static INSTALLED: OnceLock<Arc<Sink>> = OnceLock::new();

/// Configures the `workgraph` logger tree for the process. The first call installs the handlers
/// (stderr and the rotating file); every call sets the level, as Python's does.
pub fn configure_logging(settings: &LogSettings) -> Result<(), WgError> {
    if let Some(sink) = INSTALLED.get() {
        *lock(&sink.level) = settings.level;
        return Ok(());
    }
    let layer = json_layer(settings)?;
    if INSTALLED.set(Arc::clone(&layer.sink)).is_err() {
        return Ok(());
    }
    tracing::subscriber::set_global_default(Registry::default().with(layer))
        .map_err(|err| WgError::workgraph(format!("cannot install the log subscriber: {err}")))
}

/// A layer writing O14 lines as `settings` says, for callers that compose their own subscriber
/// (tests use it with `tracing::subscriber::with_default`).
pub fn json_layer(settings: &LogSettings) -> Result<JsonLayer, WgError> {
    let file = match &settings.log_dir {
        Some(dir) => Some(Mutex::new(
            RotatingFile::open(&dir.join(LOG_FILE_NAME), MAX_BYTES, BACKUP_COUNT).map_err(
                |err| {
                    WgError::workgraph(format!(
                        "cannot open the log file in {}: {err}",
                        dir.display()
                    ))
                },
            )?,
        )),
        None => None,
    };
    Ok(JsonLayer {
        sink: Arc::new(Sink {
            level: Mutex::new(settings.level),
            stderr: settings.stderr,
            file,
        }),
    })
}

/// The `tracing` layer of [`json_layer`].
#[derive(Debug, Clone)]
pub struct JsonLayer {
    sink: Arc<Sink>,
}

#[derive(Debug)]
struct Sink {
    level: Mutex<Level>,
    stderr: bool,
    file: Option<Mutex<RotatingFile>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<S: Subscriber> Layer<S> for JsonLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if !in_logger_tree(metadata.target()) {
            return;
        }
        let level = Level::from_tracing(metadata.level());
        if !level.passes(*lock(&self.sink.level)) {
            return;
        }
        let Some(at) = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i128::try_from(elapsed.as_nanos()).ok())
            .and_then(|nanos| timestamp_from_unix_nanos(nanos).ok())
        else {
            return;
        };
        let mut fields = Fields::default();
        event.record(&mut fields);
        let record = LogRecord {
            at,
            level,
            logger: metadata.target().to_owned(),
            msg: fields.msg,
            fields: fields.extra,
            exc: None,
        };
        // A record that cannot be serialised is dropped, as `Handler.handleError` drops it.
        let Ok(line) = format_record(&record) else {
            return;
        };
        if self.sink.stderr {
            let _ = io::stderr()
                .lock()
                .write_all(format!("{line}{}", line_end(cfg!(windows))).as_bytes());
        }
        if let Some(file) = &self.sink.file {
            let _ = lock(file).write_line(&line);
        }
    }
}

/// The fields of one event: `message` is the record's `msg`, the rest are extras.
#[derive(Default)]
struct Fields {
    msg: String,
    extra: Vec<(String, Value)>,
}

impl Fields {
    fn push(&mut self, field: &Field, value: Value) {
        self.extra.push((field.name().to_owned(), value));
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            value.clone_into(&mut self.msg);
        } else {
            self.push(field, Value::Str(PyStr::from(value)));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, Value::Int(i128::from(value)));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, Value::Int(i128::from(value)));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        self.push(field, Value::Int(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, Value::Bool(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, Value::Float(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.msg = text;
        } else {
            self.push(field, Value::Str(PyStr::from(text.as_str())));
        }
    }
}

/// `RotatingFileHandler`: appends lines, rolling the file over to `.1` ... `.<backups>` before a
/// line that would take it to `max_bytes`.
#[derive(Debug)]
pub struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    backup_count: u32,
    file: Option<File>,
    /// Bytes in the current file.
    position: u64,
}

impl RotatingFile {
    /// Opens (creating it and its directory) the file at `path` for appending.
    pub fn open(path: &Path, max_bytes: u64, backup_count: u32) -> io::Result<RotatingFile> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let position = file.metadata()?.len();
        Ok(RotatingFile {
            path: path.to_path_buf(),
            max_bytes,
            backup_count,
            file: Some(file),
            position,
        })
    }

    /// Writes one line and its terminator, rolling over first when due. Only a regular file
    /// rolls over.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let regular = std::fs::metadata(&self.path).map_or(true, |meta| meta.is_file());
        if regular && should_rollover(self.position, line.chars().count() + 1, self.max_bytes) {
            self.rollover()?;
        }
        let text = format!("{line}{}", line_end(cfg!(windows)));
        let file = match self.file.as_mut() {
            Some(file) => file,
            None => self.file.insert(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            ),
        };
        file.write_all(text.as_bytes())?;
        file.flush()?;
        self.position = self
            .position
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
        Ok(())
    }

    /// `doRollover`: closes the file, shifts the backups and starts an empty file.
    fn rollover(&mut self) -> io::Result<()> {
        self.file = None;
        for (source, target) in rollover_moves(&self.path, self.backup_count) {
            if source.exists() {
                if target.exists() {
                    std::fs::remove_file(&target)?;
                }
                std::fs::rename(&source, &target)?;
            }
        }
        self.file = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.position = 0;
        Ok(())
    }
}
