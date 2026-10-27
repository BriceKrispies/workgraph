//! The verification run a stack verification starts (`VerificationService.run`): the manifest
//! and run rows, each check as a child process, and the verdict.

use std::path::{Path, PathBuf};

use crate::spec::exec::check::{
    CheckPlace, ProcessFacts, check_env, check_place, concluded, errored, log_relative,
    timeout_millis,
};
use crate::spec::exec::run::{
    Executed, LogFile, Prepared, finalize_run, prepare_run, selected_checks,
};
use serde_json::Value;
use wg_model::entity::JsonObject;
use wg_proc::{LineOptions, ProcError, ProcessSpec, base_environment, run_process};
use wg_runtimespec::RequestContext;
use wg_stacks::regression::RunResult;
use wg_types::{Id, WgError};
use wg_verify::{ApplicableCheck, CheckTier};

use super::stack_world;

use crate::spec::io_failure;
use wg_runtime::index_written;

use super::Exec;

impl Exec<'_> {
    /// `VerificationService.run` for the parent task of `stack_id`: the full tier, on the revisions
    /// of `entries`, restricted to the checks `keep` admits. `stack` is merged into the run's
    /// events.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_stack_verification(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        parent_id: Id,
        entries: Vec<JsonObject>,
        keep: &dyn Fn(&ApplicableCheck) -> bool,
        stack: Value,
        cancel: &dyn Fn() -> bool,
    ) -> Result<RunResult, WgError> {
        let prepared = self
            .0
            .runtime
            .decide(
                context,
                |session| {
                    let world = stack_world(session, stack_id)?;
                    let configs = session.verification_configs(
                        world
                            .tasks
                            .task(parent_id)
                            .map(|task| task.workspace_id)
                            .ok_or_else(|| {
                                WgError::not_found(format!("task {parent_id} not found"))
                            })?,
                    )?;
                    Ok((world, configs))
                },
                |(world, configs), stamp| {
                    let checks =
                        selected_checks(world, configs, parent_id, Some(CheckTier::Full), keep)?;
                    prepare_run(
                        world,
                        stamp,
                        parent_id,
                        checks,
                        entries.clone(),
                        Some(CheckTier::Full),
                        Some(stack.clone()),
                    )
                },
            )?
            .reply;
        let executed = prepared
            .checks
            .iter()
            .map(|check| self.execute_check(&prepared, check, cancel))
            .collect::<Result<Vec<Executed>, WgError>>()?;
        let judged = self
            .0
            .runtime
            .decide_then(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| finalize_run(world, stamp, &prepared, &executed),
                index_written,
            )?
            .reply;
        Ok(judged.result)
    }

    /// `_execute_check`: one check as a child process in the revision's checkout.
    fn execute_check(
        &self,
        prepared: &Prepared,
        check: &ApplicableCheck,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Executed, WgError> {
        let spec = &check.spec;
        let run_dir = Path::new(self.0.paths.runs_dir().as_str())
            .join("verification")
            .join(prepared.run_id.to_string());
        std::fs::create_dir_all(&run_dir).map_err(|err| io_failure(&run_dir, &err))?;
        let manifest_path = run_dir.join("manifest.json");
        let manifest = serde_json::to_string_pretty(&prepared.entries)
            .map_err(|err| WgError::workgraph(err.to_string()))?;
        std::fs::write(&manifest_path, manifest).map_err(|err| io_failure(&manifest_path, &err))?;
        let (cwd, repository_id) = match check_place(&prepared.entries, spec) {
            CheckPlace::Missing(done) => return Ok(*done),
            CheckPlace::Run { cwd, repository_id } => (
                cwd.map_or_else(|| run_dir.clone(), PathBuf::from),
                repository_id,
            ),
        };
        let relative = log_relative(prepared.workspace_id, prepared.run_id, &spec.name);
        let log_path = Path::new(self.0.paths.artifacts_dir().as_str()).join(&relative);
        let mut process = ProcessSpec::new(spec.command.clone());
        process.cwd = Some(cwd);
        process.env = Some(base_environment(
            check_env(prepared, spec, &manifest_path.to_string_lossy()),
            false,
        ));
        process.timeout_ms = Some(timeout_millis(spec.timeout_seconds));
        let options = LineOptions {
            log_file: Some(log_path.clone()),
            ..LineOptions::default()
        };
        let finished = match run_process(&process, options, Some(cancel)) {
            Ok(finished) => finished,
            Err(err @ (ProcError::LogFile { .. } | ProcError::Wait(_))) => {
                return Err(WgError::workgraph(err.to_string()));
            }
            Err(err) => return Ok(errored(spec, repository_id, &err.to_string())),
        };
        let logged = std::fs::read(&log_path).ok();
        let facts = ProcessFacts {
            ok: finished.ok(),
            cancelled: finished.cancelled,
            timed_out: finished.timed_out,
            exit_code: finished.exit_code.map(i64::from),
            duration_ms: i64::try_from(finished.duration_ms).unwrap_or(i64::MAX),
            failure_lines: logged.as_deref().map_or_else(
                || finished.tail.clone(),
                |bytes| {
                    String::from_utf8_lossy(bytes)
                        .lines()
                        .map(str::to_owned)
                        .collect()
                },
            ),
            tail: finished.tail,
        };
        let log = logged.map(|bytes| LogFile {
            name: log_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: relative.clone(),
            size_bytes: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            sha256: wg_hash::sha256_hex(&bytes),
            media_type: "text/plain".to_owned(),
        });
        Ok(concluded(spec, repository_id, &facts, log))
    }
}
