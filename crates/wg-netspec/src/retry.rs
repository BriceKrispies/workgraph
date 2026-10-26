//! Retry decisions (`integrations/llm/http_common.py` for the providers, `JiraClient._get` for
//! Jira). Each function is a function of one attempt's result and its number, so the shell keeps
//! the sleeping and the sending.

use wg_model::{EffectError, EffectErrorKind, HttpResponse};

use crate::error::ProviderError;
use crate::text::head;

/// Statuses the providers retry (`RETRYABLE_STATUS`).
pub const PROVIDER_RETRYABLE: [u16; 7] = [408, 425, 429, 500, 502, 503, 504];
/// One request plus up to three retries for Jira (`MAX_ATTEMPTS`).
pub const JIRA_MAX_ATTEMPTS: u32 = 4;
/// The longest Jira waits between two attempts, in seconds (`MAX_BACKOFF_SECONDS`).
pub const JIRA_MAX_BACKOFF_SECONDS: f64 = 30.0;
/// The first Jira backoff, in seconds; it doubles per attempt.
pub const JIRA_BASE_BACKOFF_SECONDS: f64 = 0.5;

/// What one attempt produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Attempt {
    /// The server answered with this status.
    Status {
        code: u16,
        retry_after: Option<String>,
        body: String,
    },
    /// The connection failed or timed out (`httpx.TransportError`): the request may be retried.
    Transport(String),
    /// The call could not be made at all (a malformed URL, say): it is not retried.
    Failed(String),
}

impl Attempt {
    /// The attempt a transport result stands for.
    pub fn of(result: &Result<HttpResponse, EffectError>) -> Self {
        match result {
            Ok(response) => Self::Status {
                code: response.status,
                retry_after: header(&response.headers, "Retry-After"),
                body: response.body.clone(),
            },
            Err(err) => match err.kind {
                EffectErrorKind::Unavailable
                | EffectErrorKind::TimedOut
                | EffectErrorKind::NotFound => Self::Transport(err.message.clone()),
                EffectErrorKind::Failed => Self::Failed(err.message.clone()),
            },
        }
    }

    /// The attempt as an error message gives it: `HTTP <code>: <body>` or the transport message.
    pub fn describe(&self) -> String {
        match self {
            Self::Status { code, body, .. } => format!("HTTP {code}: {}", head(body, 200)),
            Self::Transport(message) | Self::Failed(message) => message.clone(),
        }
    }
}

/// What the caller does after an attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The attempt's result is final.
    Finished,
    /// Sleep for `delay_seconds`, then attempt again.
    Retry { delay_seconds: f64 },
}

/// The exponential backoff of a provider call (`max_retries`, `backoff_seconds`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backoff {
    pub max_retries: u32,
    pub base_seconds: f64,
}

/// Whether a provider retries this attempt: a transport failure or a retryable status.
pub fn provider_retryable(attempt: &Attempt) -> bool {
    match attempt {
        Attempt::Transport(_) => true,
        Attempt::Status { code, .. } => PROVIDER_RETRYABLE.contains(code),
        Attempt::Failed(_) => false,
    }
}

/// The step after attempt number `attempt_no` (starting at 1) of a provider call. Retries stop
/// once `max_retries` retries have been spent; the wait doubles from `base_seconds`.
pub fn provider_step(attempt: &Attempt, attempt_no: u32, backoff: Backoff) -> Step {
    if !provider_retryable(attempt) || attempt_no > backoff.max_retries {
        return Step::Finished;
    }
    Step::Retry {
        delay_seconds: backoff.base_seconds * exponent(attempt_no),
    }
}

/// Whether Jira retries this attempt: 429 and 5xx except 501, which will not succeed later.
/// Transport failures are not retried.
pub fn jira_retryable(attempt: &Attempt) -> bool {
    match attempt {
        Attempt::Status { code, .. } => *code == 429 || (*code >= 500 && *code != 501),
        Attempt::Transport(_) | Attempt::Failed(_) => false,
    }
}

/// The step after attempt number `attempt_no` of a Jira request.
pub fn jira_step(attempt: &Attempt, attempt_no: u32) -> Step {
    if !jira_retryable(attempt) || attempt_no >= JIRA_MAX_ATTEMPTS {
        return Step::Finished;
    }
    let retry_after = match attempt {
        Attempt::Status { retry_after, .. } => retry_after.as_deref(),
        Attempt::Transport(_) | Attempt::Failed(_) => None,
    };
    Step::Retry {
        delay_seconds: jira_delay(retry_after, attempt_no),
    }
}

/// The wait before the next Jira attempt: `Retry-After` in seconds when the server sent a number,
/// else exponential backoff; both capped at [`JIRA_MAX_BACKOFF_SECONDS`].
pub fn jira_delay(retry_after: Option<&str>, attempt_no: u32) -> f64 {
    let from_header = retry_after
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .map(|seconds| seconds.clamp(0.0, JIRA_MAX_BACKOFF_SECONDS));
    from_header.unwrap_or_else(|| {
        (JIRA_BASE_BACKOFF_SECONDS * exponent(attempt_no)).min(JIRA_MAX_BACKOFF_SECONDS)
    })
}

/// The provider's final answer once its retries are spent. A transport failure, or a retryable
/// status that stayed one, becomes `ProviderUnavailable`; any other response passes through.
pub fn settle(
    provider: &str,
    attempts: u32,
    result: Result<HttpResponse, EffectError>,
) -> Result<HttpResponse, ProviderError> {
    let attempt = Attempt::of(&result);
    match (result, provider_retryable(&attempt)) {
        (_, true) => Err(unreachable(provider, attempts, &attempt)),
        (Ok(response), false) => Ok(response),
        (Err(err), false) => Err(ProviderError::Unavailable(err.message)),
    }
}

/// `ProviderUnavailableError` for a provider that could not be reached after `attempts` tries.
pub fn unreachable(provider: &str, attempts: u32, attempt: &Attempt) -> ProviderError {
    ProviderError::Unavailable(format!(
        "{provider} is unreachable after {attempts} attempt(s): {}",
        attempt.describe()
    ))
}

/// The value of header `name`, matched without regard to case.
pub fn header(headers: &rpds::VectorSync<(String, String)>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// `2^(attempt_no - 1)` as a float: the backoff multiplier of attempt `attempt_no`.
fn exponent(attempt_no: u32) -> f64 {
    2f64.powi(i32::try_from(attempt_no.saturating_sub(1)).unwrap_or(i32::MAX))
}
