//! The artifact rows of a run (`core/services/artifacts.py`): the prompt an agent was given and
//! its log. The file bytes are written by the shell; the row, the file name and the path under
//! the artifacts directory are decided here.

use serde_json::{Map, Value};
use wg_hash::sha256_hex;
use wg_model::entity::Artifact;
use wg_types::{Id, Timestamp};

/// The longest file name an artifact keeps.
const NAME_LIMIT: usize = 150;

/// `safe_name`: anything but letters, digits, dot, underscore and dash becomes one underscore, the
/// ends lose dots and underscores, an empty name is `artifact`.
pub fn safe_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .fold((String::new(), false), |(mut text, in_run), c| {
            let kept = c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
            if kept {
                text.push(c);
            } else if !in_run {
                text.push('_');
            }
            (text, !kept)
        })
        .0;
    let trimmed = replaced.trim_matches(['.', '_']);
    let named = if trimmed.is_empty() {
        "artifact"
    } else {
        trimmed
    };
    named.chars().take(NAME_LIMIT).collect()
}

/// The path of a stored artifact below the artifacts directory: `<workspace>/<artifact>/<name>`.
pub fn stored_path(workspace: Id, artifact: Id, name: &str) -> String {
    format!("{workspace}/{artifact}/{name}")
}

/// What an artifact is made of besides its bytes.
#[derive(Debug, Clone)]
pub struct ArtifactSpec {
    pub workspace_id: Id,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub kind: String,
    pub name: String,
    pub media_type: String,
    pub retention: String,
    pub metadata: Map<String, Value>,
}

/// The row of an artifact whose file `content` is stored at `path` (relative to the artifacts
/// directory).
pub fn artifact_row(
    id: Id,
    at: Timestamp,
    spec: ArtifactSpec,
    path: String,
    content: &[u8],
) -> Artifact {
    Artifact {
        id,
        workspace_id: spec.workspace_id,
        task_id: spec.task_id,
        attempt_id: spec.attempt_id,
        kind: spec.kind,
        name: spec.name,
        media_type: spec.media_type,
        path,
        size_bytes: i64::try_from(content.len()).unwrap_or(i64::MAX),
        sha256: sha256_hex(content),
        retention: spec.retention,
        purged_at: None,
        metadata: spec.metadata,
        created_at: at,
    }
}
