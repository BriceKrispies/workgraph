//! The retry loop of the HTTP clients: send, ask `wg-netspec` what to do with the attempt, sleep,
//! repeat. The decisions are pure; this is only the sequencing of effects.

use wg_model::{EffectError, HttpCall, HttpResponse};
use wg_netspec::ProviderError;
use wg_netspec::retry::{Attempt, Backoff, Step, provider_step, settle};

use crate::host::Net;

/// Sends `call` and retries as `step` decides after each attempt. Returns the last result and
/// the number of attempts made.
pub fn send_with_retries(
    net: &mut Net,
    call: &HttpCall,
    step: impl Fn(&Attempt, u32) -> Step,
) -> (Result<HttpResponse, EffectError>, u32) {
    let mut attempt_no = 1;
    loop {
        let result = net.perform(call);
        match step(&Attempt::of(&result), attempt_no) {
            Step::Finished => return (result, attempt_no),
            Step::Retry { delay_seconds } => {
                net.pause(delay_seconds);
                attempt_no += 1;
            }
        }
    }
}

/// One provider request with its backoff. A transport failure, or a retryable status that
/// outlasts the retries, is `ProviderUnavailable`. Any other response is returned as it is,
/// with the number of attempts made.
pub fn provider_request(
    net: &mut Net,
    call: &HttpCall,
    provider: &str,
    backoff: Backoff,
) -> (Result<HttpResponse, ProviderError>, u32) {
    let (result, attempts) =
        send_with_retries(net, call, |attempt, n| provider_step(attempt, n, backoff));
    (settle(provider, attempts, result), attempts)
}
