//! The pure half of the application context (`app.py`'s `Workgraph`): the API token text, the
//! configuration error messages, and the statements and rules of default-workspace resolution.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use wg_config::ConfigError;
use wg_storecodec::{Raw, Statement};
use wg_types::{ErrorCode, Id, WgError, parse_id};
use wg_validate::ValidationError;

/// `secrets.token_urlsafe(32)`: the bytes as unpadded URL-safe base64 (43 characters for 32).
pub fn token_urlsafe(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A file the application context could not read, write or create.
pub fn io_error(what: &str, path: &std::path::Path, err: &std::io::Error) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("cannot {what} {}: {err}", path.display()),
    )
}

/// `ConfigStore` failing on the file at `path`: `config_error`, worded as Python words it.
pub fn config_failure(path: &str, err: ConfigError) -> WgError {
    match err {
        ConfigError::InvalidToml(detail) => WgError::new(
            ErrorCode::ConfigError,
            format!("{path} is not valid TOML: {detail}"),
        ),
        ConfigError::Validation(errors) => {
            WgError::new(ErrorCode::ConfigError, format!("{path} failed validation"))
                .with_detail("errors", validation_details(&errors))
        }
        ConfigError::Exception(name) => WgError::new(ErrorCode::WorkgraphError, name),
    }
}

/// `ConfigStore.effective` failing after the merge.
pub fn merged_config_failure(slug: &str, err: ConfigError) -> WgError {
    match err {
        ConfigError::Validation(errors) => WgError::new(
            ErrorCode::ConfigError,
            format!("workspace overrides for {slug} produce an invalid configuration"),
        )
        .with_detail("errors", validation_details(&errors)),
        other => config_failure(slug, other),
    }
}

fn validation_details(errors: &[ValidationError]) -> Value {
    Value::Array(errors.iter().map(ValidationError::to_json).collect())
}

/// `session.get(Workspace, ref)`: the workspace whose id is the reference text exactly.
pub fn workspace_by_id(reference: &str) -> Statement {
    Statement {
        sql: "SELECT id, slug FROM workspaces WHERE id = ?".to_owned(),
        params: vec![Raw::Text(reference.to_owned())],
    }
}

/// `select(Workspace).where(Workspace.slug == ref)`.
pub fn workspace_by_slug(slug: &str) -> Statement {
    Statement {
        sql: "SELECT id, slug FROM workspaces WHERE slug = ?".to_owned(),
        params: vec![Raw::Text(slug.to_owned())],
    }
}

/// `select(Workspace.id).where(Workspace.archived_at.is_(None)).limit(2)`.
pub fn unarchived_workspaces() -> Statement {
    Statement {
        sql: "SELECT id, slug FROM workspaces WHERE archived_at IS NULL LIMIT 2".to_owned(),
        params: Vec::new(),
    }
}

/// `is_uuid(ref)`: whether `resolve.workspace` first tries the reference as an id.
pub fn is_uuid(reference: &str) -> bool {
    parse_id(reference).is_ok()
}

/// A workspace row read by one of the statements above: `(id, slug)`.
pub fn workspace_rows(rows: &[Vec<Raw>]) -> Result<Vec<(Id, String)>, WgError> {
    rows.iter()
        .map(|row| match row.as_slice() {
            [Raw::Text(id), Raw::Text(slug)] => Ok((parse_id(id)?, slug.clone())),
            _ => Err(WgError::new(
                ErrorCode::WorkgraphError,
                "unexpected workspace row shape",
            )),
        })
        .collect()
}

/// `resolve.workspace` finding nothing.
pub fn workspace_not_found(reference: &str) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("workspace {} not found", wg_model::py_repr_str(reference)),
    )
    .with_detail("ref", Value::String(reference.to_owned()))
}

/// `Workgraph.workspace_slug` finding nothing.
pub fn workspace_id_not_found(workspace_id: &str) -> WgError {
    WgError::new(
        ErrorCode::NotFound,
        format!("workspace {workspace_id} not found"),
    )
}

/// `Workgraph.default_workspace_id` without a configured slug: the only unarchived workspace.
pub fn only_workspace(unarchived: &[(Id, String)]) -> Result<Id, WgError> {
    match unarchived {
        [(id, _)] => Ok(*id),
        _ => Err(WgError::new(
            ErrorCode::NotFound,
            "no default workspace configured; pass --workspace or run `workgraph workspace use <slug>`",
        )),
    }
}

/// The `default_workspace` member of a loaded global configuration, when set and non-empty.
pub fn configured_default_workspace(config: &serde_json::Map<String, Value>) -> Option<String> {
    config
        .get("default_workspace")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_has_python_length_and_alphabet() {
        let token = token_urlsafe(&[0xfb; 32]);
        assert_eq!(token.len(), 43);
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn default_workspace_rules() {
        let a = wg_model::derive_id(&[1; 32], 0);
        let b = wg_model::derive_id(&[1; 32], 1);
        assert_eq!(only_workspace(&[(a, "a".to_owned())]).unwrap(), a);
        let none = only_workspace(&[]).unwrap_err();
        assert_eq!(none.code, ErrorCode::NotFound);
        assert!(only_workspace(&[(a, "a".to_owned()), (b, "b".to_owned())]).is_err());
        let missing = workspace_not_found("ops");
        assert_eq!(missing.message, "workspace 'ops' not found");
        assert!(is_uuid(&a.to_string()) && !is_uuid("ops"));
    }

    #[test]
    fn config_messages() {
        let err = config_failure("C:\\wg\\config.toml", ConfigError::InvalidToml("x".into()));
        assert_eq!(err.message, "C:\\wg\\config.toml is not valid TOML: x");
        assert_eq!(err.code, ErrorCode::ConfigError);
        let merged = merged_config_failure("ops", ConfigError::Validation(Vec::new()));
        assert_eq!(
            merged.message,
            "workspace overrides for ops produce an invalid configuration"
        );
    }
}
