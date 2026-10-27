//! The application entry points of `init`, `doctor` and `config` (IF-02): what
//! `cli/commands/core.py` asks of `Workgraph`.
//!
//! Each one gathers facts from the files and the database and hands them to the pure functions of
//! `spec::setup`; the decisions about what they mean are made there.

use std::path::{Path, PathBuf};

use crate::spec::setup::{
    DoctorFacts, EventProjection, InitFacts, InitRequest, doctor_checks, doctor_result, healthy,
    init_result, init_workspace, init_workspace_request, migrate_result, paths_result, show_json,
    workspace_file,
};
use serde_json::{Map, Value as Json};
use wg_agents::{
    AgentRunner, ClaudeCodeProvider, ClaudeConfig, CommandAgentProvider, CommandConfig,
    describe_adapter,
};
use wg_config::ConfigFiles;
use wg_events::{task_states, verify_task_projection};
use wg_json::Value;
use wg_model::StoredEvent;
use wg_net::{Endpoint, Net, Ollama, OpenAiCompatible};
use wg_registry::providers::{
    ProviderHealth, agent_adapters, entry_type, llm_health, llm_providers, wanted_models,
};
use wg_runtimespec::RequestContext;
use wg_runtimespec::app::{config_failure, io_error};
use wg_store::head_revision;
use wg_storecodec::EventQuery;
use wg_types::{ErrorCode, WgError};
use wg_wire::convention::json_value;
use wg_wire::dto::{agents as agents_dto, llm as llm_dto};

use wg_runtime::App;
use wg_runtime::write_atomic;

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

/// `config.toml` and every `workspaces/*.toml`, as texts.
pub fn config_files(app: &App) -> Result<ConfigFiles, WgError> {
    let global = PathBuf::from(app.paths.config_file().as_str());
    let workspaces = toml_files(Path::new(app.paths.workspace_config_dir().as_str()))?
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
    app: &mut App,
    slug: &str,
    overrides: &Map<String, Json>,
) -> Result<(), WgError> {
    let path = PathBuf::from(workspace_file(&app.paths, slug));
    let text = wg_config::render(&wg_config::strip_none(overrides));
    write_atomic(&path, text.as_bytes()).map_err(|err| io_error("write", &path, &err))
}

/// `config set`'s starting point: the configuration dump, or with a workspace the overrides
/// of that workspace without their `null` members.
pub fn config_set_base(app: &App, workspace: Option<&str>) -> Result<Map<String, Json>, WgError> {
    match workspace {
        None => app.config(),
        Some(slug) => {
            let path = PathBuf::from(workspace_file(&app.paths, slug));
            config_files(app)?
                .load_workspace(slug)
                .map(|dump| wg_config::strip_none(&dump))
                .map_err(|err| config_failure(&path.display().to_string(), err))
        }
    }
}

/// `config show`: the workspace (else the configured default), its effective configuration
/// and the files it comes from.
pub fn config_show(app: &mut App, workspace: Option<&str>) -> Result<Json, WgError> {
    let default = app.config()?.get("default_workspace").cloned();
    let slug = workspace.map(str::to_owned).or_else(|| {
        default
            .as_ref()
            .and_then(Json::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    });
    let config = app.effective_config(slug.as_deref())?;
    let global = app.paths.config_file();
    let workspace_file = slug.as_deref().map(|slug| {
        PathBuf::from(workspace_file(&app.paths, slug))
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
pub fn config_paths(app: &mut App) -> Value {
    paths_result(&[
        ("config_file", app.paths.config_file().as_str().to_owned()),
        ("config_dir", app.paths.config_dir().as_str().to_owned()),
        ("data_dir", app.paths.data_dir().as_str().to_owned()),
        ("log_dir", app.paths.log_dir().as_str().to_owned()),
        ("database", app.paths.database_file().as_str().to_owned()),
    ])
}

/// `ConfigStore.validate_files`: the problems found, none when every file is valid.
pub fn config_problems(app: &App) -> Result<Vec<String>, WgError> {
    let path = app.paths.config_file();
    let global = PathBuf::from(path.as_str()).display().to_string();
    let workspace_dir = PathBuf::from(app.paths.workspace_config_dir().as_str());
    config_files(app)?
        .validate_files(&global, |slug| {
            workspace_dir
                .join(format!("{slug}.toml"))
                .display()
                .to_string()
        })
        .map_err(|err| config_failure(path.as_str(), err))
}

/// `ConfigStore.migrate_files`: rewrites the files in an older schema; the result lists them.
pub fn config_migrate(app: &App) -> Result<Value, WgError> {
    let path = app.paths.config_file();
    let (migrated, rewritten) = config_files(app)?
        .migrate_files()
        .map_err(|err| config_failure(path.as_str(), err))?;
    let config_dir = PathBuf::from(app.paths.config_dir().as_str());
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
    app: &mut App,
    context: &RequestContext,
    request: &InitRequest<'_>,
) -> Result<Value, WgError> {
    if !Path::new(app.paths.config_file().as_str()).exists() {
        let config = app.config()?;
        app.save_config(&config)?;
    }
    app.api_token(true)?;
    let created = if request.no_workspace {
        None
    } else {
        let dump = init_workspace_request(request)?;
        let created = app
            .runtime
            .decide(
                context,
                |session| session.workspaces(),
                |existing, stamp| init_workspace(existing, stamp, &dump),
            )?
            .reply;
        if let Some(slug) = &created {
            adopt_workspace(app, slug, true)?;
        }
        created
    };
    let revision = app.runtime.store.current_revision()?;
    let default = app.config()?.get("default_workspace").cloned();
    Ok(init_result(&InitFacts {
        config_dir: app.paths.config_dir().as_str(),
        data_dir: app.paths.data_dir().as_str(),
        database: app.paths.database_file().as_str(),
        schema_revision: revision.as_deref(),
        workspace_created: created.as_deref(),
        default_workspace: default.as_ref().and_then(Json::as_str),
    }))
}

/// What `WorkspaceService.create` does after the row is written: the workspace's override
/// file, and the workspace as the configured default when `make_default` is set or none is.
pub(crate) fn adopt_workspace(
    app: &mut App,
    slug: &str,
    make_default: bool,
) -> Result<(), WgError> {
    if !PathBuf::from(workspace_file(&app.paths, slug)).exists() {
        save_workspace_overrides(app, slug, &wg_config::default_overrides())?;
    }
    if make_default
        || app
            .config()?
            .get("default_workspace")
            .is_none_or(Json::is_null)
    {
        save_default_workspace(app, slug)?;
    }
    Ok(())
}

/// `WorkspaceService.set_default`'s write: `slug` as the configured default workspace.
pub(crate) fn save_default_workspace(app: &mut App, slug: &str) -> Result<(), WgError> {
    let mut config = app.config()?;
    config.insert(
        "default_workspace".to_owned(),
        Json::String(slug.to_owned()),
    );
    app.save_config(&config)
}

/// The probes `doctor` makes of the LLM providers and the agent adapters. [`LiveProbe`] reaches
/// the endpoints, the executables and the offline heuristic provider; tests answer from their own.
pub trait HealthProbe {
    /// `list_models()` of the provider that `entry` configures.
    fn list_models(&mut self, name: &str, entry: &Map<String, Json>)
    -> Result<Vec<String>, String>;
    /// `check_model(model)` of the provider that `entry` configures: whether it serves the model,
    /// and the sentence that says so.
    fn check_model(&mut self, name: &str, entry: &Map<String, Json>, model: &str)
    -> (bool, String);
    /// `check_available()` of the agent adapter that `entry` configures.
    fn agent(&mut self, name: &str, entry: &Map<String, Json>) -> (bool, String);
}

/// The probes against the real endpoints, executables and the heuristic provider.
pub struct LiveProbe;

/// The default base URL of an Ollama provider (`OllamaProviderConfig.base_url`).
const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// The credential an entry names in `api_key_env`, read from the environment (`secret_from_env`).
fn api_key(entry: &Map<String, Json>) -> Option<String> {
    entry
        .get("api_key_env")
        .and_then(Json::as_str)
        .and_then(|name| std::env::var(name).ok())
}

fn ollama(entry: &Map<String, Json>) -> Ollama {
    Ollama {
        endpoint: Endpoint::from_config(entry, OLLAMA_URL, api_key(entry)),
        net: Net::default(),
    }
}

fn openai_compatible(name: &str, entry: &Map<String, Json>) -> OpenAiCompatible {
    OpenAiCompatible {
        name: name.to_owned(),
        endpoint: Endpoint::from_config(entry, "", api_key(entry)),
        net: Net::default(),
    }
}

impl HealthProbe for LiveProbe {
    fn list_models(
        &mut self,
        name: &str,
        entry: &Map<String, Json>,
    ) -> Result<Vec<String>, String> {
        match entry_type(entry) {
            "ollama" => ollama(entry)
                .list_models()
                .map_err(|error| error.message().to_owned()),
            "openai_compatible" => openai_compatible(name, entry)
                .list_models()
                .map_err(|error| error.message().to_owned()),
            _ => Ok(wg_intel::list_models()),
        }
    }

    fn check_model(
        &mut self,
        name: &str,
        entry: &Map<String, Json>,
        model: &str,
    ) -> (bool, String) {
        match entry_type(entry) {
            "ollama" => ollama(entry).check_model(model),
            "openai_compatible" => openai_compatible(name, entry).check_model(model),
            _ => wg_intel::check_model(model),
        }
    }

    fn agent(&mut self, name: &str, entry: &Map<String, Json>) -> (bool, String) {
        if entry_type(entry) == "claude_code" {
            ClaudeCodeProvider::new(name, ClaudeConfig::from_config(entry)).check_available()
        } else {
            match CommandAgentProvider::new(name, CommandConfig::from_config(entry)) {
                Ok(provider) => provider.check_available(),
                Err(error) => (false, error.to_string()),
            }
        }
    }
}

/// The health rows of `doctor`: a name with its health, for the LLM providers and for the agent
/// adapters.
pub type ProviderRows = (Vec<(String, ProviderHealth)>, Vec<(String, ProviderHealth)>);

/// `LLMRegistry.health` over a dumped configuration: each LLM provider with its health, in
/// configuration order. The probes are made here and the decisions over their answers are
/// `wg-registry`'s.
pub fn llm_rows<P: HealthProbe>(
    config: &Map<String, Json>,
    probe: &mut P,
) -> Vec<(String, ProviderHealth)> {
    llm_providers(config)
        .into_iter()
        .map(|(name, entry)| {
            let wanted = wanted_models(config, name);
            let listing = wanted.is_empty().then(|| probe.list_models(name, entry));
            let checks: Vec<(bool, String)> = wanted
                .iter()
                .map(|model| probe.check_model(name, entry, model))
                .collect();
            (name.to_owned(), llm_health(listing, &checks))
        })
        .collect()
}

/// `AgentRegistry.health` over a dumped configuration: each agent adapter with its availability,
/// in configuration order.
pub fn agent_rows<P: HealthProbe>(
    config: &Map<String, Json>,
    probe: &mut P,
) -> Vec<(String, ProviderHealth)> {
    agent_adapters(config)
        .into_iter()
        .map(|(name, entry)| {
            let (available, detail) = probe.agent(name, entry);
            (name.to_owned(), ProviderHealth { available, detail })
        })
        .collect()
}

/// `LLMRegistry.health` and `AgentRegistry.health` over a dumped configuration: each LLM provider
/// and each agent adapter with its health, both in configuration order.
pub fn provider_health<P: HealthProbe>(config: &Map<String, Json>, probe: &mut P) -> ProviderRows {
    (llm_rows(config, probe), agent_rows(config, probe))
}

/// The body of `GET /api/v1/llm/health` (`LLMRegistry.health`): a dict keyed by provider name,
/// each value `{available, detail}`.
pub fn llm_report<P: HealthProbe>(config: &Map<String, Json>, probe: &mut P) -> Value {
    let rows = llm_rows(config, probe);
    llm_dto::health(
        rows.iter()
            .map(|(name, row)| (name.as_str(), row.available, row.detail.as_str())),
    )
}

/// The body of `GET /api/v1/agents` (`AgentRegistry.describe`): one entry per agent adapter in
/// configuration order, with the probe's availability and detail, the provider's capabilities and
/// its non-secret configuration snapshot. An invalid argument template fails the listing, as
/// `CommandAgentProvider.__init__` raises in Python.
pub fn agent_listing<P: HealthProbe>(
    config: &Map<String, Json>,
    probe: &mut P,
) -> Result<Value, WgError> {
    agent_adapters(config)
        .into_iter()
        .map(|(name, entry)| {
            let (available, detail) = probe.agent(name, entry);
            let described = describe_adapter(entry)
                .map_err(|error| WgError::new(ErrorCode::ConfigError, error.to_string()))?;
            Ok(agents_dto::agent(
                name,
                described.adapter,
                available,
                &detail,
                json_value(&described.capabilities),
                json_value(&described.config),
            ))
        })
        .collect::<Result<Vec<Value>, WgError>>()
        .map(agents_dto::agents)
}

/// `GET /api/v1/agents` against the live adapters and the configuration of the application.
pub fn agents_document(app: &App) -> Result<Value, WgError> {
    agent_listing(&app.config()?, &mut LiveProbe)
}

/// `GET /api/v1/llm/health` against the live providers and the configuration of the application.
pub fn llm_health_document(app: &App) -> Result<Value, WgError> {
    Ok(llm_report(&app.config()?, &mut LiveProbe))
}

/// `doctor`: the checks the native build can run, with the live probes of the providers and
/// adapters. `languages` are the tree-sitter grammars that load, which the caller asks of the
/// code parser.
pub fn doctor(
    app: &mut App,
    languages: Vec<String>,
    verify_events: bool,
) -> Result<Diagnosis, WgError> {
    doctor_with(app, languages, verify_events, &mut LiveProbe)
}

/// [`doctor`] with the provider and adapter probes supplied by the caller.
pub fn doctor_with<P: HealthProbe>(
    app: &mut App,
    languages: Vec<String>,
    verify_events: bool,
    probe: &mut P,
) -> Result<Diagnosis, WgError> {
    let config = app.config()?;
    let (llm, agents) = provider_health(&config, probe);
    let event_projections = if verify_events {
        Some(event_projections(app)?)
    } else {
        None
    };
    let facts = DoctorFacts {
        git: git_version(),
        current_revision: app.runtime.store.current_revision()?,
        head_revision: head_revision().map(str::to_owned),
        integrity: app.runtime.store.integrity_check()?,
        journal_mode: app.runtime.store.journal_mode()?,
        config_problems: config_problems(app)?,
        config_file: app.paths.config_file().as_str().to_owned(),
        api_token_exists: Path::new(app.paths.api_token_file().as_str()).exists(),
        api_token_file: app.paths.api_token_file().as_str().to_owned(),
        languages,
        llm,
        agents,
        execution_enabled: config
            .get("execution")
            .and_then(|execution| execution.get("enabled"))
            .and_then(Json::as_bool)
            .unwrap_or(false),
        live_workers: app
            .runtime
            .store
            .read(|session| session.workers())?
            .iter()
            .filter(|worker| worker.status == "running")
            .count(),
        dead_jobs: app
            .runtime
            .store
            .read(|session| session.job_stats(None))?
            .get("dead")
            .copied()
            .unwrap_or(0),
        spooled_events: spooled_count(Path::new(app.paths.spool_dir().as_str())),
        event_projections,
    };
    let checks = doctor_checks(&facts);
    Ok(Diagnosis {
        result: doctor_result(&checks),
        healthy: healthy(&checks),
    })
}

/// `events verify` for each active workspace (`history.verify_task_projection`), in slug
/// order: each workspace's task events replayed against its task rows.
fn event_projections(app: &mut App) -> Result<Vec<EventProjection>, WgError> {
    let workspaces = app.runtime.store.read(|session| session.workspaces())?;
    workspaces
        .into_iter()
        .filter(|workspace| workspace.archived_at.is_none())
        .map(|workspace| {
            let (events, tasks) = app.runtime.store.read(|session| {
                let events = session.query_events(&EventQuery {
                    workspace_id: Some(workspace.id),
                    entity_type: Some("task".to_owned()),
                    limit: 10_000_000,
                    ..EventQuery::default()
                })?;
                let world = session.task_world(Some(workspace.id), &[])?;
                Ok((events, world.tasks))
            })?;
            let report = verify_task_projection(
                events.into_iter().map(StoredEvent::from).collect(),
                &task_states(&tasks),
            )
            .map_err(|error| WgError::new(ErrorCode::WorkgraphError, error.to_string()))?;
            Ok(EventProjection {
                slug: workspace.slug,
                tasks: report.tasks,
                events: report.events,
                mismatches: report.mismatches.into_iter().map(|(key, _)| key).collect(),
            })
        })
        .collect()
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
