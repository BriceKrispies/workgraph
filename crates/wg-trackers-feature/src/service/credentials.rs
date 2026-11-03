//! Shell: where a Jira connection's token comes from, an environment variable or a command that
//! prints it. Tokens are held in memory only, never logged, stored or put in an error message.

use std::time::{Duration, Instant};

use wg_net::TokenSource;
use wg_netspec::TrackerError;
use wg_proc::{CaptureLimits, ProcessSpec, run_capture};

use crate::spec::credentials::token_command_error;

/// A command is re-run at most this often, so an endpoint that always answers 401 cannot loop it.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// A token read once from the environment; it cannot be refreshed.
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn token(&mut self) -> Result<String, TrackerError> {
        Ok(self.0.clone())
    }

    fn refresh(&mut self) -> Result<bool, TrackerError> {
        Ok(false)
    }
}

/// A token printed on stdout by a configured command, such as an OAuth helper.
pub struct CommandToken {
    argv: Vec<String>,
    /// The executable as configured, for messages.
    name: String,
    timeout: Duration,
    token: Option<String>,
    fetched_at: Option<Instant>,
}

impl CommandToken {
    #[must_use]
    pub fn new(argv: Vec<String>, timeout_seconds: f64) -> Self {
        Self {
            name: argv.first().cloned().unwrap_or_default(),
            argv,
            timeout: Duration::from_secs_f64(timeout_seconds.max(0.0)),
            token: None,
            fetched_at: None,
        }
    }

    fn fetch(&mut self) -> Result<String, TrackerError> {
        let mut spec = ProcessSpec::new(self.argv.clone());
        // The whole environment is inherited: helpers commonly need the user's profile and
        // credential store.
        spec.timeout_ms = Some(u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX));
        let result = run_capture(&spec, CaptureLimits::default(), None).map_err(|err| {
            token_command_error(
                format!("token command {} could not start: {err}", self.name),
                "",
            )
        })?;
        let stderr = String::from_utf8_lossy(&result.stderr);
        if result.timed_out {
            return Err(token_command_error(
                format!(
                    "token command {} timed out after {}s",
                    self.name,
                    self.timeout.as_secs_f64()
                ),
                "",
            ));
        }
        if result.exit_code != Some(0) {
            let code = result
                .exit_code
                .map_or_else(|| "none".to_owned(), |code| code.to_string());
            return Err(token_command_error(
                format!("token command {} exited with code {code}", self.name),
                &stderr,
            ));
        }
        let stdout = String::from_utf8_lossy(&result.stdout);
        let token = stdout.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err(token_command_error(
                format!(
                    "token command {} must print exactly one token on stdout",
                    self.name
                ),
                &stderr,
            ));
        }
        self.token = Some(token.to_owned());
        self.fetched_at = Some(Instant::now());
        Ok(token.to_owned())
    }
}

impl TokenSource for CommandToken {
    fn token(&mut self) -> Result<String, TrackerError> {
        match &self.token {
            Some(token) => Ok(token.clone()),
            None => self.fetch(),
        }
    }

    fn refresh(&mut self) -> Result<bool, TrackerError> {
        if self
            .fetched_at
            .is_some_and(|at| at.elapsed() < MIN_REFRESH_INTERVAL)
        {
            return Ok(false);
        }
        self.fetch().map(|_| true)
    }
}
