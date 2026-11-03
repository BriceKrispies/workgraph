//! Pure: the errors of a Jira token command. They name the command and quote the end of its
//! stderr, never its stdout, which is where the token is.

use wg_netspec::TrackerError;

/// How much of a failed command's stderr an error message keeps.
const STDERR_TAIL_CHARS: usize = 400;

/// A token-command failure, with the tail of the command's stderr when it wrote any.
#[must_use]
pub fn token_command_error(message: String, stderr: &str) -> TrackerError {
    let stderr = stderr.trim();
    let skip = stderr.chars().count().saturating_sub(STDERR_TAIL_CHARS);
    let tail: String = stderr.chars().skip(skip).collect();
    TrackerError {
        message: if tail.is_empty() {
            message
        } else {
            format!("{message}: {tail}")
        },
        status: None,
    }
}
