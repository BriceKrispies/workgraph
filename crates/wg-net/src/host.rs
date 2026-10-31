//! The process side of the HTTP clients: the socket (`ureq`), the sleeping, the monotonic clock,
//! the time zones (`jiff`) and the log (`tracing`). Each method is one effect; what is sent, when
//! to retry and what the answers mean is `wg-netspec`'s.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rpds::VectorSync;
use wg_model::{EffectError, EffectErrorKind, HttpCall, HttpMethod, HttpResponse};
use wg_netspec::text::os_transport_message;

/// A function that waits `seconds`. Tests replace the real wait with one that records it.
pub type Sleeper = Box<dyn FnMut(f64) + Send>;

/// The clock reading that [`Net::monotonic_ms`] counts from.
static CLOCK_START: OnceLock<Instant> = OnceLock::new();

/// The process host: real sockets, real sleeps, the system's time zones.
#[derive(Default)]
pub struct Net {
    /// Replaces the real sleep between retries when set.
    pub sleep: Option<Sleeper>,
}

impl Net {
    /// Performs `call` once, bounded by its timeout. An HTTP error status is a response.
    pub fn perform(&mut self, call: &HttpCall) -> Result<HttpResponse, EffectError> {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(call.timeout_ms.map(Duration::from_millis))
            .build();
        let agent: ureq::Agent = config.into();
        let method = match call.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
        };
        let mut builder = ureq::http::Request::builder()
            .method(method)
            .uri(call.url.as_str());
        for (name, value) in call.headers.iter() {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let body = call.body.clone().unwrap_or_default().into_bytes();
        let request = builder.body(body).map_err(|err| EffectError {
            kind: EffectErrorKind::Failed,
            message: format!("cannot build the request for {}: {err}", call.url),
        })?;
        let mut response = agent.run(request).map_err(|err| EffectError {
            kind: match err {
                ureq::Error::Timeout(_) => EffectErrorKind::TimedOut,
                ureq::Error::HostNotFound => EffectErrorKind::NotFound,
                ureq::Error::ConnectionFailed | ureq::Error::Io(_) => EffectErrorKind::Unavailable,
                _ => EffectErrorKind::Failed,
            },
            message: match &err {
                ureq::Error::Io(io) => match io.raw_os_error() {
                    Some(code) => os_transport_message(cfg!(windows), code, &io.to_string()),
                    None => err.to_string(),
                },
                other => other.to_string(),
            },
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect::<VectorSync<(String, String)>>();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|err| EffectError {
                kind: EffectErrorKind::Unavailable,
                message: err.to_string(),
            })?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Waits `seconds` before the next attempt, or hands the wait to [`Net::sleep`] when set.
    pub fn pause(&mut self, seconds: f64) {
        match self.sleep.as_mut() {
            Some(sleep) => sleep(seconds),
            None => {
                if seconds.is_finite() && seconds > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(seconds));
                }
            }
        }
    }

    /// Milliseconds since the first reading, on a clock that never goes back.
    pub fn monotonic_ms(&mut self) -> u64 {
        let start = CLOCK_START.get_or_init(Instant::now);
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Whether `name` is a time zone the system knows.
    pub fn zone_known(&mut self, name: &str) -> bool {
        TimeZone::get(name).is_ok()
    }

    /// Seconds east of UTC in zone `name` at the instant `epoch_seconds`; 0 when either is unknown.
    pub fn zone_offset(&mut self, name: &str, epoch_seconds: i64) -> i64 {
        let zone = TimeZone::get(name).ok();
        let instant = Timestamp::from_second(epoch_seconds).ok();
        match (zone, instant) {
            (Some(zone), Some(instant)) => i64::from(zone.to_offset(instant).seconds()),
            _ => 0,
        }
    }

    /// Logs a warning.
    pub fn warn(&mut self, message: &str) {
        tracing::warn!("{message}");
    }
}
