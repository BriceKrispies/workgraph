//! The configuration store: `config.toml` and the workspace files, as texts.
//!
//! Reading, validating, migrating and merging are pure functions of these texts. The shell crate
//! reads the files into a [`ConfigFiles`], asks for the result, and writes the texts back.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::ConfigError;
use crate::model::{validate_global, validate_overrides};
use crate::pyfmt::{errors_repr, value_repr};
use crate::toml_doc::parse_document;
use crate::value::deep_merge;
use crate::{CONFIG_SCHEMA_VERSION, strip_none};
use wg_types::pyval::truthy;

/// The texts of `config.toml` and of each `workspaces/<slug>.toml`. A missing file is `None` or an
/// absent key, and reads as an empty document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigFiles {
    /// The text of `config.toml`.
    pub global: Option<String>,
    /// The text of each workspace file, keyed by slug.
    pub workspaces: BTreeMap<String, String>,
}

impl ConfigFiles {
    /// The global configuration, migrated and validated, as its dump with defaults filled in.
    pub fn load_global(&self) -> Result<Map<String, Value>, ConfigError> {
        let mut raw = read(self.global.as_deref())?;
        migrate(&mut raw)?;
        validate_global(&Value::Object(raw)).map_err(ConfigError::Validation)
    }

    /// The overrides of one workspace, migrated and validated, as their dump (`null` members
    /// included).
    pub fn load_workspace(&self, slug: &str) -> Result<Map<String, Value>, ConfigError> {
        let mut raw = read(self.workspaces.get(slug).map(String::as_str))?;
        migrate(&mut raw)?;
        validate_overrides(&Value::Object(raw)).map_err(ConfigError::Validation)
    }

    /// The configuration in force for a workspace: the global one with the workspace's overrides
    /// deep-merged over it, validated again. Without a slug, the global configuration.
    pub fn effective(&self, slug: Option<&str>) -> Result<Map<String, Value>, ConfigError> {
        let base = self.load_global()?;
        let Some(slug) = slug else {
            return Ok(base);
        };
        let mut overrides = strip_none(&self.load_workspace(slug)?);
        overrides.remove("schema_version");
        if overrides.is_empty() {
            return Ok(base);
        }
        let merged = deep_merge(&base, &overrides);
        validate_global(&Value::Object(merged)).map_err(ConfigError::Validation)
    }

    /// Human-readable problems across all files, as `ConfigStore.validate_files` words them: each
    /// is the failure's message, a colon, and its details (the Pydantic errors, or nothing).
    /// `global_path` names `config.toml` and `workspace_path` names a workspace's file, as Python
    /// prints their paths. Empty when every file is valid. A non-validation exception (an
    /// unreadable `schema_version`) is returned as an error, as Python raises it.
    pub fn validate_files(
        &self,
        global_path: &str,
        workspace_path: impl Fn(&str) -> String,
    ) -> Result<Vec<String>, ConfigError> {
        let mut problems = Vec::new();
        if let Err(error) = self.load_global() {
            problems.push(file_problem(error, global_path)?);
        }
        let mut slugs: Vec<&String> = self.workspaces.keys().collect();
        slugs.sort_by_key(|slug| format!("{slug}.toml"));
        for slug in slugs {
            // Python's `effective` loads the global file first, so a broken one is reported again
            // for every workspace; then the workspace file; then the merged configuration.
            let checked = self
                .load_global()
                .map_err(|error| (global_path.to_owned(), error))
                .and_then(|_| {
                    self.load_workspace(slug)
                        .map_err(|error| (workspace_path(slug), error))
                });
            match checked {
                Err((path, error)) => problems.push(file_problem(error, &path)?),
                Ok(_) => {
                    if let Err(error) = self.effective(Some(slug)) {
                        problems.push(merged_problem(error, slug)?);
                    }
                }
            }
        }
        Ok(problems)
    }

    /// Rewrites every file whose `schema_version` is below the current one, in the current schema.
    /// Returns the migrated store and the rewritten paths, relative to the configuration directory.
    /// The store it was called on is left as it was.
    pub fn migrate_files(&self) -> Result<(ConfigFiles, Vec<String>), ConfigError> {
        let mut migrated = self.clone();
        let mut rewritten = Vec::new();
        if let Some(text) = &self.global {
            let raw = parse_document(text)?;
            if version_of(&raw)? < CONFIG_SCHEMA_VERSION {
                let config = self.load_global()?;
                migrated.global = Some(crate::render(&strip_none(&config)));
                rewritten.push("config.toml".to_string());
            }
        }
        for (slug, text) in &self.workspaces {
            let raw = parse_document(text)?;
            if version_of(&raw)? < CONFIG_SCHEMA_VERSION {
                let overrides = self.load_workspace(slug)?;
                let rendered = crate::render(&strip_none(&overrides));
                migrated.workspaces.insert(slug.clone(), rendered);
                rewritten.push(format!("workspaces/{slug}.toml"));
            }
        }
        Ok((migrated, rewritten))
    }
}

/// Parses a file's text, or an empty document when the file is missing.
fn read(text: Option<&str>) -> Result<Map<String, Value>, ConfigError> {
    match text {
        Some(text) => parse_document(text),
        None => Ok(Map::new()),
    }
}

/// `_schema_version(raw)`: the document's `schema_version`, default 1. Only a real integer is
/// accepted; `bool`, float and string values are a `ConfigError`, as Python raises them.
fn version_of(raw: &Map<String, Value>) -> Result<i64, ConfigError> {
    let value = raw
        .get("schema_version")
        .cloned()
        .unwrap_or_else(|| Value::from(1));
    value
        .as_i64()
        .ok_or(ConfigError::InvalidSchemaVersion(value))
}

/// The problem line of a failure in the file at `path`: `ConfigStore`'s message for it, then the
/// details `validate_files` prints after it. An exception is not a problem; it is returned.
fn file_problem(error: ConfigError, path: &str) -> Result<String, ConfigError> {
    match error {
        ConfigError::Exception(_) => Err(error),
        ConfigError::InvalidToml(detail) => Ok(format!("{path} is not valid TOML: {detail}: ")),
        ConfigError::InvalidSchemaVersion(value) => Ok(format!(
            "{path} has an invalid schema_version: expected an integer, got {}: ",
            value_repr(&value)
        )),
        ConfigError::Validation(errors) => Ok(format!(
            "{path} failed validation: {}",
            errors_repr(&errors)
        )),
    }
}

/// The problem line of a workspace whose overrides, merged over the global configuration, fail.
fn merged_problem(error: ConfigError, slug: &str) -> Result<String, ConfigError> {
    match error {
        ConfigError::Validation(errors) => Ok(format!(
            "workspace overrides for {slug} produce an invalid configuration: {}",
            errors_repr(&errors)
        )),
        other => file_problem(other, slug),
    }
}

/// `_migrate`: a version 1 document kept agent execution under `agents.enabled`; it moves to
/// `execution.enabled`. The version is then set to the current one.
fn migrate(raw: &mut Map<String, Value>) -> Result<(), ConfigError> {
    let mut version = version_of(raw)?;
    if version < 2 {
        let enabled = raw
            .get_mut("agents")
            .and_then(Value::as_object_mut)
            .and_then(|agents| agents.remove("enabled"));
        if let Some(enabled) = enabled {
            let execution = raw
                .entry("execution")
                .or_insert_with(|| Value::Object(Map::new()));
            let Some(execution) = execution.as_object_mut() else {
                return Err(ConfigError::Exception("TypeError"));
            };
            execution.insert("enabled".to_string(), Value::Bool(truthy(&enabled)));
        }
        version = CONFIG_SCHEMA_VERSION;
    }
    raw.insert("schema_version".to_string(), Value::from(version));
    Ok(())
}
