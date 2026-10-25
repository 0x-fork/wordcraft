//! Regressions where main's "a command that changes nothing is not an edit" (#23) meets the
//! migration's input-method application-change and state-revision tracking.
//!
//! Kept in its own module so the merged `tests.rs` is not touched.

use serde_json::json;

use crate::Session;

fn s() -> Session {
    Session::new(wordcraft_doc::Document::new())
}

fn run(s: &mut Session, id: &str, v: serde_json::Value) -> serde_json::Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// An *unclassified* no-op (`text.backspace` at the very start of the body) is not an edit and
/// must not look like one to the input method: no effect flags, no application-change signal, no
/// settled-revision move, and no history/dirty/revision side effects.
#[test]
fn a_no_op_command_is_not_an_ime_state_change() {
    let mut s = s();
    run(&mut s, "text.insert", json!({"text": "x"}));
    run(&mut s, "caret.docStart", json!({}));
    assert!(s.pending.is_none(), "caret movement dropped any pending formatting");
    s.take_pending_ime_application_change();
    let revision = s.ime_state_revision().expect("an IME revision");
    let rev = s.rev();
    let depth = s.undo_depth();
    let dirty = s.dirty;

    run(&mut s, "text.backspace", json!({}));

    let outcome = s.ime_edit_outcome();
    assert!(!outcome.classified, "the handler leaves this no-op unclassified: {outcome:?}");
    assert!(!outcome.content_changed && !outcome.state_changed, "{outcome:?}");
    assert!(!s.take_pending_ime_application_change(), "a no-op is not an application change");
    assert_eq!(s.ime_state_revision(), Some(revision), "the settled IME revision did not move");
    assert_eq!((s.undo_depth(), s.dirty, s.rev()), (depth, dirty, rev));
}

/// `document.setText` with identical text replaces the body under the same document id, so the
/// IME replacement epoch must advance — but because nothing observable changed it is not an edit
/// and must not signal an application-origin change (main's #23 no-op rule).
#[test]
fn a_same_text_set_text_advances_the_body_epoch_without_an_application_change() {
    let mut s = s();
    run(&mut s, "document.setText", json!({"text": "Same body"}));
    s.take_pending_ime_application_change();
    let epoch = s.ime_body_epoch();
    let document_id = s.document_id();
    let rev = s.rev();
    let depth = s.undo_depth();

    run(&mut s, "document.setText", json!({"text": "Same body"}));

    assert!(s.ime_body_epoch() > epoch, "the body is replaced again under the same document id");
    assert_eq!(s.document_id(), document_id, "the document id itself is unchanged");
    assert!(!s.take_pending_ime_application_change(), "an identical setText is not an application change");
    assert_eq!((s.undo_depth(), s.rev()), (depth, rev));
}
