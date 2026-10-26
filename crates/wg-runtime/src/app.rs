//! [`App`]: the application context every interface opens (`app.py`'s `Workgraph`): the resolved
//! paths, the configuration files, the database and the runtime that decides on it.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use wg_config::ConfigFiles;
use wg_config::paths::{AppPaths, Environment, Os};
use wg_runtimespec::app::{
    config_failure, configured_default_workspace, io_error, is_uuid, merged_config_failure,
    only_workspace, token_urlsafe, unarchived_workspaces, workspace_by_id, workspace_by_slug,
    workspace_id_not_found, workspace_not_found, workspace_rows,
};
use wg_store::{Location, Session, Store};
use wg_types::{ErrorCode, Id, WgError};

use crate::cycle::Runtime;
use crate::effects::{Interpreter, write_atomic};
use crate::stamp::{SystemStamps, draw};

/// The application context.
pub struct App {
    pub paths: AppPaths,
    pub runtime: Runtime,
}

/// The variables the path rules read, from this process's environment.
pub fn environment() -> Environment {
    let var = |key: &str| std::env::var(key).ok();
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    Environment {
        os,
        workgraph_home: var("WORKGRAPH_HOME"),
        home: if cfg!(windows) {
            var("USERPROFILE")
        } else {
            var("HOME")
        },
        local_appdata: var("LOCALAPPDATA"),
        xdg_config_home: var("XDG_CONFIG_HOME"),
        xdg_data_home: var("XDG_DATA_HOME"),
        xdg_state_home: var("XDG_STATE_HOME"),
    }
}

/// Reads a text file; a missing file is `None`.
fn read_optional(path: &Path) -> Result<Option<String>, WgError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error("read", path, &err)),
    }
}

impl App {
    /// `Workgraph.open(home)`: resolves the paths (`home`, else `WORKGRAPH_HOME`, else the
    /// platform directories), creates the directories and opens the database.
    ///
    /// The schema migrations are not run here yet: they arrive with the migration runner.
    pub fn open(home: Option<&Path>) -> Result<App, WgError> {
        let env = environment();
        let root = home
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| env.workgraph_home.clone().filter(|text| !text.is_empty()));
        // `Path(root).expanduser().resolve()`: a relative root is taken from the working
        // directory; `~` is expanded by the path rules.
        let root = match root {
            Some(text) if !text.starts_with('~') => Some(
                std::path::absolute(&text)
                    .map_err(|err| io_error("resolve", Path::new(&text), &err))?
                    .to_string_lossy()
                    .into_owned(),
            ),
            other => other,
        };
        let paths = AppPaths::resolve(&Environment {
            workgraph_home: root,
            ..env
        })
        .map_err(|err| WgError::new(ErrorCode::ConfigError, err.to_string()))?;
        for dir in [
            paths.config_dir(),
            paths.workspace_config_dir(),
            paths.data_dir(),
            paths.log_dir(),
            paths.artifacts_dir(),
            paths.worktrees_dir(),
            paths.spool_dir(),
            paths.backups_dir(),
            paths.runs_dir(),
        ] {
            let dir = PathBuf::from(dir.as_str());
            std::fs::create_dir_all(&dir).map_err(|err| io_error("create", &dir, &err))?;
        }
        let store = Store::open(&Location::File(PathBuf::from(
            paths.database_file().as_str(),
        )))?;
        Ok(App {
            paths,
            runtime: Runtime {
                store,
                stamps: Box::new(SystemStamps),
                effects: Interpreter::default(),
            },
        })
    }

    /// `Workgraph.config`: `config.toml`, migrated and validated, with defaults filled in.
    pub fn config(&self) -> Result<Map<String, Value>, WgError> {
        let path = PathBuf::from(self.paths.config_file().as_str());
        let files = ConfigFiles {
            global: read_optional(&path)?,
            ..ConfigFiles::default()
        };
        files
            .load_global()
            .map_err(|err| config_failure(&path.display().to_string(), err))
    }

    /// `Workgraph.effective_config(slug)`: the global configuration with the workspace's
    /// overrides merged over it.
    pub fn effective_config(&self, slug: Option<&str>) -> Result<Map<String, Value>, WgError> {
        let global_path = PathBuf::from(self.paths.config_file().as_str());
        let global = read_optional(&global_path)?;
        let Some(slug) = slug.filter(|slug| !slug.is_empty()) else {
            return self.config();
        };
        let workspace_path =
            PathBuf::from(self.paths.workspace_config_dir().as_str()).join(format!("{slug}.toml"));
        let files = ConfigFiles {
            global,
            workspaces: read_optional(&workspace_path)?
                .map(|text| (slug.to_owned(), text))
                .into_iter()
                .collect(),
        };
        files
            .load_global()
            .map_err(|err| config_failure(&global_path.display().to_string(), err))?;
        files
            .load_workspace(slug)
            .map_err(|err| config_failure(&workspace_path.display().to_string(), err))?;
        files
            .effective(Some(slug))
            .map_err(|err| merged_config_failure(slug, err))
    }

    /// `Workgraph.api_token(create=...)`: the token in `<config>/api-token`, generated as
    /// `secrets.token_urlsafe(32)` and written atomically when missing and `create` is set.
    pub fn api_token(&self, create: bool) -> Result<String, WgError> {
        let path = PathBuf::from(self.paths.api_token_file().as_str());
        if let Some(text) = read_optional(&path)? {
            return Ok(text.trim().to_owned());
        }
        if !create {
            return Err(WgError::new(
                ErrorCode::ConfigError,
                "API token has not been generated; run `workgraph init`",
            ));
        }
        let (_, entropy) = draw()?;
        let token = token_urlsafe(&entropy);
        write_atomic(&path, token.as_bytes()).map_err(|err| io_error("write", &path, &err))?;
        restrict_permissions(&path);
        Ok(token)
    }

    /// `Workgraph.default_workspace_id()`: the configured default workspace, else the only
    /// unarchived one.
    pub fn default_workspace_id(&mut self) -> Result<Id, WgError> {
        let configured = configured_default_workspace(&self.config()?);
        self.runtime.store.read(|session| match &configured {
            Some(slug) => resolve_workspace(session, slug),
            None => only_workspace(&workspace_rows(&session.rows(&unarchived_workspaces())?)?),
        })
    }

    /// `resolve.workspace(session, ref)`: by id when the reference is a UUID, else by slug.
    pub fn workspace_id(&mut self, reference: &str) -> Result<Id, WgError> {
        self.runtime
            .store
            .read(|session| resolve_workspace(session, reference))
    }

    /// `Workgraph.workspace_slug(workspace_id)`.
    pub fn workspace_slug(&mut self, workspace_id: &str) -> Result<String, WgError> {
        self.runtime.store.read(|session| {
            workspace_rows(&session.rows(&workspace_by_id(workspace_id))?)?
                .into_iter()
                .next()
                .map(|(_, slug)| slug)
                .ok_or_else(|| workspace_id_not_found(workspace_id))
        })
    }
}

/// `resolve.workspace`: by id when `reference` is a UUID, then by slug.
fn resolve_workspace(session: &mut Session<'_>, reference: &str) -> Result<Id, WgError> {
    let by_id = if is_uuid(reference) {
        workspace_rows(&session.rows(&workspace_by_id(reference))?)?
    } else {
        Vec::new()
    };
    let found = match by_id.into_iter().next() {
        Some(row) => Some(row),
        None => workspace_rows(&session.rows(&workspace_by_slug(reference))?)?
            .into_iter()
            .next(),
    };
    found
        .map(|(id, _)| id)
        .ok_or_else(|| workspace_not_found(reference))
}

/// `token_file.chmod(0o600)`, best effort (a no-op on Windows).
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::metadata(path);
    }
}
