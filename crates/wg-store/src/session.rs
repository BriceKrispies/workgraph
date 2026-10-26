//! [`Session`]: the open transaction a store body works in.
//!
//! The pure half (`wg-storecodec`) builds every statement as a value and decodes every row from
//! values; a session only binds a statement's parameters, runs it and hands the rows back as
//! values. It takes `&mut self` throughout: a session is one exclusive cursor over the
//! transaction, so two bodies cannot interleave on it.

use rusqlite::Connection;
use rusqlite::params_from_iter;
use rusqlite::types::Value as Sql;
use wg_model::{Decision, EventDraft, Row, RowKey, Write};
use wg_storecodec::{
    EventQuery, EventRecord, Raw, Statement, TableRow, append_statement, decode_event,
    latest_seq_statement, select_by_key, update_failure, version_probe, write_statement,
};
use wg_types::{ErrorCode, WgError};

/// Maps a rusqlite result to a domain result: a constraint violation (unique, foreign key, check,
/// immutability trigger) is a `conflict`, a locked database `resource_busy`, anything else a
/// `workgraph_error`. A macro rather than a function so the classification sits at each call.
macro_rules! db {
    ($result:expr) => {
        $result.map_err(|err| {
            let kind = match &err {
                rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
                    rusqlite::ErrorCode::ConstraintViolation => {
                        wg_storecodec::DbFailure::Constraint
                    }
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                        wg_storecodec::DbFailure::Busy
                    }
                    _ => wg_storecodec::DbFailure::Other,
                },
                _ => wg_storecodec::DbFailure::Other,
            };
            wg_storecodec::db_error(kind, &err.to_string())
        })
    };
}

pub(crate) use db;

/// What one applied decision did to the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The `seq` of each appended event, in decision order.
    pub event_seqs: Vec<i64>,
}

/// A value to bind.
macro_rules! to_sql {
    ($param:expr) => {
        match $param {
            Raw::Null => Sql::Null,
            Raw::Int(value) => Sql::Integer(*value),
            Raw::Real(value) => Sql::Real(*value),
            Raw::Text(text) => Sql::Text(text.clone()),
            Raw::Blob(bytes) => Sql::Blob(bytes.clone()),
        }
    };
}

/// A value read.
macro_rules! from_sql {
    ($value:expr) => {
        match $value {
            Sql::Null => Raw::Null,
            Sql::Integer(value) => Raw::Int(value),
            Sql::Real(value) => Raw::Real(value),
            Sql::Text(text) => Raw::Text(text),
            Sql::Blob(bytes) => Raw::Blob(bytes),
        }
    };
}

/// An open transaction (see the module documentation).
pub struct Session<'c> {
    pub(crate) conn: &'c Connection,
}

impl Session<'_> {
    /// Runs a statement and returns the number of rows it changed.
    pub fn run(&mut self, statement: &Statement) -> Result<usize, WgError> {
        let mut prepared = db!(self.conn.prepare_cached(&statement.sql))?;
        db!(prepared.execute(params_from_iter(
            statement.params.iter().map(|param| to_sql!(param))
        )))
    }

    /// Runs a query and returns every row as values, in column order.
    pub fn rows(&mut self, statement: &Statement) -> Result<Vec<Vec<Raw>>, WgError> {
        let mut prepared = db!(self.conn.prepare_cached(&statement.sql))?;
        let width = prepared.column_count();
        let mut result = db!(prepared.query(params_from_iter(
            statement.params.iter().map(|param| to_sql!(param))
        )))?;
        let mut out = Vec::new();
        while let Some(row) = db!(result.next())? {
            let values = (0..width)
                .map(|index| row.get::<_, Sql>(index).map(|value| from_sql!(value)))
                .collect::<Result<Vec<_>, _>>();
            out.push(db!(values)?);
        }
        Ok(out)
    }

    /// Runs one or more statements of raw SQL with no parameters (setup and scripts).
    pub fn execute_batch(&mut self, sql: &str) -> Result<(), WgError> {
        db!(self.conn.execute_batch(sql))
    }

    /// Applies one write. An insert clashing with a key or unique constraint fails with
    /// `conflict`; an update that finds no row fails with `not_found`, or `version_conflict` when
    /// guarded and the row exists at another version; a delete of an absent row does nothing.
    pub fn apply_write(&mut self, write: &Write) -> Result<(), WgError> {
        let changed = self.run(&write_statement(write)?)?;
        match write {
            Write::Update {
                row,
                expected_version,
            } if changed == 0 => {
                let key = row.key();
                let actual = match expected_version {
                    Some(_) => self
                        .rows(&version_probe(&key))?
                        .first()
                        .and_then(|values| values.first())
                        .and_then(|value| match value {
                            Raw::Int(version) => Some(*version),
                            _ => None,
                        }),
                    None => None,
                };
                Err(update_failure(row.table(), *expected_version, actual))
            }
            _ => Ok(()),
        }
    }

    /// Appends one event and returns its `seq`.
    pub fn append_event(&mut self, draft: &EventDraft) -> Result<i64, WgError> {
        self.run(&append_statement(draft)?)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Applies a decision's writes in order, then appends its events in order. Effects are not the
    /// store's: the runtime performs them after the transaction commits.
    pub fn apply_decision<R>(&mut self, decision: &Decision<R>) -> Result<Applied, WgError> {
        for write in &decision.writes {
            self.apply_write(write)?;
        }
        let event_seqs = decision
            .events
            .iter()
            .map(|event| self.append_event(event))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Applied { event_seqs })
    }

    /// Runs an [`EventQuery`].
    pub fn query_events(&mut self, query: &EventQuery) -> Result<Vec<EventRecord>, WgError> {
        self.rows(&query.statement())?
            .iter()
            .map(|values| decode_event(values))
            .collect()
    }

    /// The highest event `seq`, or 0 for an empty log.
    pub fn latest_seq(&mut self) -> Result<i64, WgError> {
        match self
            .rows(&latest_seq_statement())?
            .first()
            .and_then(|r| r.first())
        {
            Some(Raw::Int(seq)) => Ok(*seq),
            _ => Err(WgError::new(
                ErrorCode::WorkgraphError,
                "latest_seq returned no integer",
            )),
        }
    }

    /// Reads the row with primary key `key`, if it exists.
    pub fn fetch<T: TableRow>(&mut self, key: &RowKey) -> Result<Option<T>, WgError> {
        if key.table() != T::TABLE {
            return Err(WgError::new(
                ErrorCode::WorkgraphError,
                format!(
                    "key for {} used to read {}",
                    key.table().name(),
                    T::TABLE.name()
                ),
            ));
        }
        self.rows(&select_by_key(key))?
            .first()
            .map(|values| T::from_raw(values))
            .transpose()
    }

    /// Reads any row by key, through its table's codec.
    pub fn fetch_row(&mut self, key: &RowKey) -> Result<Option<Row>, WgError> {
        use wg_model::entity::*;
        macro_rules! by_table {
            ($($variant:ident),+ $(,)?) => {
                match key.table() {
                    $(wg_model::Table::$variant => self.fetch::<$variant>(key)?.map(Row::from),)+
                }
            };
        }
        Ok(by_table!(
            Workspace,
            Project,
            ProjectRepository,
            Repository,
            RepositoryCheckout,
            RepositoryRemote,
            RepositoryRef,
            RepositoryDependency,
            Task,
            TaskFile,
            TaskTag,
            TaskLink,
            TaskComment,
            TaskDependency,
            TaskRelation,
            TaskProject,
            TaskRepository,
            ExternalItem,
            TaskDecision,
            Commit,
            CommitFileChange,
            CommitSymbol,
            CommitAnalysis,
            CommitEquivalence,
            CommitTaskLink,
            GitObjectGap,
            TaskChangeSurface,
            VerificationConfig,
            VerificationRun,
            VerificationCheckResult,
            TaskAcceptance,
            Classification,
            ClassificationGoldLabel,
            ClassificationEvaluation,
            TaskEmbedding,
            AutomationRule,
            AutomationRun,
            Notification,
            Report,
            Finding,
            AgentSession,
            SessionBinding,
            SubagentRun,
            AgentInternalTask,
            AgentProfile,
            HookEvent,
            ToolEvent,
            Experiment,
            ExperimentTrial,
            PredictorFit,
            AgentAttempt,
            RevisionManifest,
            ManagedWorktree,
            ResourceLease,
            ChangeSet,
            ChangeSetItem,
            Artifact,
            Stack,
            StackRepository,
            StackItem,
            Job,
            JobRun,
            JobResourceClaim,
            Worker,
            SystemCursor,
        ))
    }
}
