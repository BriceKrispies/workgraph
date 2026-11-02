//! WG-4067: a pending item is reported as waiting, with the reason it cannot merge, and the note is
//! cleared once it no longer applies.

use std::error::Error;

use wg_stacks::queue::{ItemDetail, set_waiting, waiting_note};
use wg_types::StackItemState as S;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn pending_item_without_head_is_waiting_for_work() {
    let text = waiting_note(S::Pending, false, "WG-7").expect("a pending item with no head waits");
    assert!(text.contains("WG-7"));
    assert!(text.contains("attempt adopt"));
}

#[test]
fn pending_item_with_head_is_waiting_for_readmit() {
    let text = waiting_note(S::Pending, true, "WG-7").expect("a pending item with a head waits");
    assert!(text.contains("WG-7"));
    assert!(text.contains("stack readmit"));
}

#[test]
fn items_past_pending_are_not_waiting() {
    for state in [S::Ready, S::Merged, S::Conflict, S::Ejected, S::Verified] {
        assert!(waiting_note(state, false, "WG-7").is_none(), "{state:?}");
        assert!(waiting_note(state, true, "WG-7").is_none(), "{state:?}");
    }
}

#[test]
fn set_waiting_sets_clears_and_keeps_other_keys() -> TestResult {
    let mut detail = ItemDetail::load(Some(r#"{"merges":{"skein":{"head":"abc"}}}"#));
    let note = waiting_note(S::Pending, false, "WG-7").expect("waiting");

    assert!(
        set_waiting(&mut detail, Some(&note)),
        "first set changes the detail"
    );
    assert!(
        !set_waiting(&mut detail, Some(&note)),
        "same note is no change"
    );
    let value = detail.to_value();
    assert_eq!(value["waiting"].as_str(), Some(note.as_str()));
    assert_eq!(value["merges"]["skein"]["head"], "abc");

    assert!(
        set_waiting(&mut detail, None),
        "clearing changes the detail"
    );
    assert!(
        !set_waiting(&mut detail, None),
        "clearing twice is no change"
    );
    assert!(detail.to_value().get("waiting").is_none());
    assert_eq!(detail.to_value()["merges"]["skein"]["head"], "abc");
    Ok(())
}
