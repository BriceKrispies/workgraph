//! The notification output of `GET /ws/{ws}/notifications`: a dict the handler builds, so its
//! datetimes are `isoformat()` strings, not Pydantic's `Z` form.

use wg_json::{Value, dict};
use wg_model::entity::automation::Notification;

use wg_wire::convention::{TimeFormat, id, json_object, optional_timestamp, text, timestamp};

/// The keys of the notification dict, in the order the handler builds it.
pub const FIELDS: [&str; 8] = [
    "id",
    "level",
    "title",
    "body",
    "source",
    "data",
    "read_at",
    "created_at",
];

/// One notification as the list returns it.
pub fn from_model(note: &Notification) -> Value {
    dict!({
        "id": id(note.id),
        "level": text(&note.level),
        "title": text(&note.title),
        "body": text(&note.body),
        "source": text(&note.source),
        "data": json_object(&note.data),
        "read_at": optional_timestamp(note.read_at, TimeFormat::Isoformat),
        "created_at": timestamp(note.created_at, TimeFormat::Isoformat),
    })
}
