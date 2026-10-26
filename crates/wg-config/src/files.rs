//! The configuration store: `config.toml` and the workspace files, as texts.
//!
//! Reading, validating, migrating and merging are pure functions of these texts. The shell crate
//! reads the files into a [`ConfigFiles`], asks for the result, and writes the texts back.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::ConfigError;
use crate::model::{validate_global, validate_overrides};
use crate::toml_doc::parse_document;
use crate::value::{deep_merge, truthy};
use crate::{CONFIG_SCHEMA_VERSION, strip_none};

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

    /// Human-readable problems across all files; empty when every file is valid. A non-validation
    /// exception (an unreadable `schema_version`) is returned as an error, as Python raises it.
    pub fn validate_files(&self) -> Result<Vec<String>, ConfigError> {
        let mut problems = Vec::new();
        match self.load_global() {
            Ok(_) => {}
            Err(error @ ConfigError::Exception(_)) => return Err(error),
            Err(error) => problems.push(error.to_string()),
        }
        for slug in self.workspaces.keys() {
            match self.effective(Some(slug)) {
                Ok(_) => {}
                Err(error @ ConfigError::Exception(_)) => return Err(error),
                Err(error) => problems.push(format!("workspaces/{slug}.toml: {error}")),
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
