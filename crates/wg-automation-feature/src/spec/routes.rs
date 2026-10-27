//! Pure: the automation and notification routes (H091 to H100, `api/routes/ops.py`): the
//! arguments read from the bodies and query parameters, and the decision that marks a notification
//! read.

use serde_json::{Map, Value as Json};
use wg_model::entity::Notification;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_types::WgError;
use wg_types::pyval::{flag, int_or};

/// The `limit` of `GET /ws/{ws}/automation/runs` when the client sends none.
pub const RUNS_LIMIT: i64 = 100;

/// The `inputs` of a rule action body (`RuleAction`'s `inputs`): an object, else none. The body
/// itself may be absent (`(body or {}).get("inputs")`).
#[must_use]
pub fn action_inputs(body: &Json) -> Map<String, Json> {
    body.get("inputs")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The arguments of `POST /ws/{ws}/automation/preview` (`PreviewBody`): the stored rule's name or
/// id, the rule document, the event id and the inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewArgs {
    pub rule: Option<String>,
    pub spec: Option<Json>,
    pub event_id: Option<String>,
    pub inputs: Map<String, Json>,
}

/// Reads the preview body. The model has checked the fields; absent ones are `None`.
#[must_use]
pub fn preview_args(body: &Json) -> PreviewArgs {
    PreviewArgs {
        rule: body.get("rule").and_then(Json::as_str).map(str::to_owned),
        spec: body.get("spec").filter(|found| found.is_object()).cloned(),
        event_id: body
            .get("event_id")
            .and_then(Json::as_str)
            .map(str::to_owned),
        inputs: body
            .get("inputs")
            .and_then(Json::as_object)
            .cloned()
            .unwrap_or_default(),
    }
}

/// The `rule`, `status` and `limit` of `GET /ws/{ws}/automation/runs`.
#[must_use]
pub fn run_args(params: &Map<String, Json>) -> (Option<&str>, Option<&str>, i64) {
    (
        params.get("rule").and_then(Json::as_str),
        params.get("status").and_then(Json::as_str),
        int_or(params.get("limit"), RUNS_LIMIT),
    )
}

/// The `unread` flag of `GET /ws/{ws}/notifications`: only unread notifications when set.
#[must_use]
pub fn unread_only(params: &Map<String, Json>) -> bool {
    flag(params.get("unread"))
}

/// `NotificationService.mark_read`: the notification with `read_at` set to the call's time. An
/// unknown notification is `NotFound`; no event is recorded, as the Python service records none.
pub fn mark_read(found: Option<Notification>, stamp: &Stamp) -> Result<Decision<()>, WgError> {
    let note = found.ok_or_else(|| WgError::not_found("notification not found"))?;
    let row = Notification {
        read_at: Some(stamp.at),
        ..note
    };
    Ok(Decision::new(()).write(Write::update(row)))
}
