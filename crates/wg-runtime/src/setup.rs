//! The application entry points of `init`, `doctor` and `config` (IF-02): what
//! `cli/commands/core.py` asks of `Workgraph`.
//!
//! Each one gathers facts from the files and the database and hands them to the pure functions of
//! `wg_runtimespec::setup`; the decisions about what they mean are made there.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value as Json};
use wg_config::ConfigFiles;
use wg_json::Value;
use wg_runtimespec::RequestContext;
use wg_runtimespec::app::{config_failure, io_error};
use wg_runtimespec::setup::{
    DoctorFacts, InitFacts, InitRequest, doctor_checks, doctor_result, healthy, init_result,
    init_workspace, init_workspace_request, migrate_result, paths_result, show_json,
    workspace_file,
};
use wg_store::head_revision;
use wg_types::WgError;

use crate::app::App;
use crate::effects::write_atomic;

/// What `doctor` found: the result to print and whether the system is healthy.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnosis {
    pub result: Value,
    pub healthy: bool,
}

/// The `*.toml` files of `dir`, sorted by name (`dir.glob("*.toml")`); none when it is missing.
fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, WgError> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml") && path.is_file())
        .collect();
    files.sort();
    Ok(files)
}

fn read_text(path: &Path) -> Result<String, WgError> {
    std::fs::read_to_string(path).map_err(|err| io_error("read", path, &err))
}

/// `git --version`, when git runs.
fn git_version() -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("--version")
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

impl App {
    /// `config.toml` and every `workspaces/*.toml`, as texts.
    pub fn config_files(&self) -> Result<ConfigFiles, WgError> {
        let global = PathBuf::from(self.paths.config_file().as_str());
        let workspaces = toml_files(Path::new(self.paths.workspace_config_dir().as_str()))?
            .into_iter()
            .filter_map(|path| {
                Some((
                    path.file_stem()?.to_string_lossy().into_owned(),
                    read_text(&path),
                ))
            })
            .map(|(slug, text)| text.map(|text| (slug, text)))
            .collect::<Result<_, _>>()?;
        Ok(ConfigFiles {
            global: std::fs::read_to_string(&global).ok(),
            workspaces,
        })
    }

    /// `ConfigStore.save_workspace_overrides`: the overrides' dump, without its `null` members.
    pub fn save_workspace_overrides(
        &self,
        slug: &str,
        overrides: &Map<String, Json>,
    ) -> Result<(), WgError> {
        let path = PathBuf::from(workspace_file(&self.paths, slug));
        let text = wg_config::render(&wg_config::strip_none(overrides));
        write_atomic(&path, text.as_bytes()).map_err(|err| io_error("write", &path, &err))
    }

    /// `config set`'s starting point: the configuration dump, or with a workspace the overrides
    /// of that workspace without their `null` members.
    pub fn config_set_base(&self, workspace: Option<&str>) -> Result<Map<String, Json>, WgError> {
        match workspace {
            None => self.config(),
            Some(slug) => {
                let path = PathBuf::from(workspace_file(&self.paths, slug));
                self.config_files()?
                    .load_workspace(slug)
                    .map(|dump| wg_config::strip_none(&dump))
                    .map_err(|err| config_failure(&path.display().to_string(), err))
            }
        }
    }

    /// `config show`: the workspace (else the configured default), its effective configuration
    /// and the files it comes from.
    pub fn config_show(&mut self, workspace: Option<&str>) -> Result<Json, WgError> {
        let default = self.config()?.get("default_workspace").cloned();
        let slug = workspace.map(str::to_owned).or_else(|| {
            default
                .as_ref()
                .and_then(Json::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        });
        let config = self.effective_config(slug.as_deref())?;
        let global = self.paths.config_file();
        let workspace_file = slug.as_deref().map(|slug| {
            PathBuf::from(workspace_file(&self.paths, slug))
                .display()
                .to_string()
        });
        Ok(show_json(
            slug.as_deref(),
            &config,
            global.as_str(),
            workspace_file.as_deref(),
        ))
    }

    /// `config path`.
    #[must_use]
    pub fn config_paths(&mut self) -> Value {
        paths_result(&[
            ("config_file", self.paths.config_file().as_str().to_owned()),
            ("config_dir", self.paths.config_dir().as_str().to_owned()),
            ("data_dir", self.paths.data_dir().as_str().to_owned()),
            ("log_dir", self.paths.log_dir().as_str().to_owned()),
            ("database", self.paths.database_file().as_str().to_owned()),
        ])
    }

    /// `ConfigStore.validate_files`: the problems found, none when every file is valid.
    pub fn config_problems(&self) -> Result<Vec<String>, WgError> {
        let path = self.paths.config_file();
        self.config_files()?
            .validate_files()
            .map_err(|err| config_failure(path.as_str(), err))
    }

    /// `ConfigStore.migrate_files`: rewrites the files in an older schema; the result lists them.
    pub fn config_migrate(&self) -> Result<Value, WgError> {
        let path = self.paths.config_file();
        let (migrated, rewritten) = self
            .config_files()?
            .migrate_files()
            .map_err(|err| config_failure(path.as_str(), err))?;
        let config_dir = PathBuf::from(self.paths.config_dir().as_str());
        let written: Vec<String> = rewritten
            .iter()
            .map(|relative| {
                let target = relative
                    .split('/')
                    .fold(config_dir.clone(), |dir, part| dir.join(part));
                let text = match relative.strip_prefix("workspaces/") {
                    Some(name) => migrated
                        .workspaces
                        .get(name.trim_end_matches(".toml"))
                        .cloned(),
                    None => migrated.global.clone(),
                };
                text.map_or(Ok(()), |text| {
                    write_atomic(&target, text.as_bytes())
                        .map_err(|err| io_error("write", &target, &err))
                })
                .map(|()| target.display().to_string())
            })
            .collect::<Result<_, _>>()?;
        Ok(migrate_result(&written))
    }

    /// `init`: writes `config.toml` when it is missing, makes sure the database is at head and the
    /// API token exists, then creates the first workspace unless `--no-workspace` or it exists.
    pub fn init(
        &mut self,
        context: &RequestContext,
        request: &InitRequest<'_>,
    ) -> Result<Value, WgError> {
        if !Path::new(self.paths.config_file().as_str()).exists() {
            let config = self.config()?;
            self.save_config(&config)?;
        }
        self.api_token(true)?;
        let created = if request.no_workspace {
            None
        } else {
            let dump = init_workspace_request(request)?;
            let created = self
                .runtime
                .decide(
                    context,
                    |session| session.workspaces(),
                    |existing, stamp| init_workspace(existing, stamp, &dump),
                )?
                .reply;
            if let Some(slug) = &created {
                self.adopt_workspace(slug)?;
            }
            created
        };
        let revision = self.runtime.store.current_revision()?;
        let default = self.config()?.get("default_workspace").cloned();
        Ok(init_result(&InitFacts {
            config_dir: self.paths.config_dir().as_str(),
            data_dir: self.paths.data_dir().as_str(),
            database: self.paths.database_file().as_str(),
            schema_revision: revision.as_deref(),
            workspace_created: created.as_deref(),
            default_workspace: default.as_ref().and_then(Json::as_str),
        }))
    }

    /// What `WorkspaceService.create(..., make_default=True)` does after the row is written: the
    /// workspace's override file, and the workspace as the configured default.
    fn adopt_workspace(&mut self, slug: &str) -> Result<(), WgError> {
        if !PathBuf::from(workspace_file(&self.paths, slug)).exists() {
            self.save_workspace_overrides(slug, &wg_config::default_overrides())?;
        }
        let mut config = self.config()?;
        config.insert(
            "default_workspace".to_owned(),
            Json::String(slug.to_owned()),
        );
        self.save_config(&config)
    }

    /// `doctor`: the checks the native build can run. `languages` are the tree-sitter grammars
    /// that load, which the caller asks of the code parser.
    pub fn doctor(&mut self, languages: Vec<String>) -> Result<Diagnosis, WgError> {
        let config = self.config()?;
        let facts = DoctorFacts {
            git: git_version(),
            current_revision: self.runtime.store.current_revision()?,
            head_revision: head_revision().map(str::to_owned),
            integrity: self.runtime.store.integrity_check()?,
            journal_mode: self.runtime.store.journal_mode()?,
            config_problems: self.config_problems()?,
            config_file: self.paths.config_file().as_str().to_owned(),
            api_token_exists: Path::new(self.paths.api_token_file().as_str()).exists(),
            api_token_file: self.paths.api_token_file().as_str().to_owned(),
            languages,
            execution_enabled: config
                .get("execution")
                .and_then(|execution| execution.get("enabled"))
                .and_then(Json::as_bool)
                .unwrap_or(false),
            live_workers: self
                .runtime
                .store
                .read(|session| session.workers())?
                .iter()
                .filter(|worker| worker.status == "running")
                .count(),
            dead_jobs: self
                .runtime
                .store
                .read(|session| session.job_stats(None))?
                .get("dead")
                .copied()
                .unwrap_or(0),
            spooled_events: spooled_count(Path::new(self.paths.spool_dir().as_str())),
        };
        let checks = doctor_checks(&facts);
        Ok(Diagnosis {
            result: doctor_result(&checks),
            healthy: healthy(&checks),
        })
    }
}

/// The number of `*.json` files in the hook spool.
fn spooled_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count()
    })
}
