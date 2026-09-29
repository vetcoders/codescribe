//! Delivery-throne and clipboard-borrow integration proof for overlay Insert.

#![cfg(target_os = "macos")]

use codescribe::controller::{
    DeliveryIntent, DeliveryRoute, delivery_intent_from_session, format_delivery_route_line,
    overlay_insert_facts, resolve_delivery_route,
};
use codescribe::os::clipboard::{ClipboardSnapshot, get_clipboard, set_clipboard};
use serial_test::serial;

struct ClipboardRestore(Option<ClipboardSnapshot>);

impl ClipboardRestore {
    fn capture() -> Self {
        Self(ClipboardSnapshot::capture().ok())
    }
}

impl Drop for ClipboardRestore {
    fn drop(&mut self) {
        if let Some(snapshot) = &self.0 {
            let _ = snapshot.restore();
        }
    }
}

fn clipboard_or_skip<T>(result: anyhow::Result<T>, action: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error)
            if format!("{error:#}")
                .contains("not supported with the current system configuration") =>
        {
            eprintln!("skipping clipboard integration test: {action}: {error:#}");
            None
        }
        Err(error) => panic!("{action}: {error:#}"),
    }
}

#[test]
#[serial]
fn foreign_insert_selects_one_route_and_borrows_clipboard_losslessly() {
    let decision = resolve_delivery_route(
        DeliveryIntent::OverlayInsert,
        overlay_insert_facts(true, false),
    );
    assert_eq!(decision.route, DeliveryRoute::ClipboardPaste);
    assert_eq!(decision.reason, "explicit_insert");
    assert_eq!(
        format_delivery_route_line(DeliveryIntent::OverlayInsert, decision, Some("Ghostty")),
        "delivery_route: intent=overlay_insert route=clipboard_paste reason=explicit_insert target=Ghostty"
    );

    let _restore_host_clipboard = ClipboardRestore::capture();
    let Some(()) = clipboard_or_skip(
        set_clipboard("clipboard-owner-sentinel"),
        "seed clipboard owner sentinel",
    ) else {
        return;
    };
    let Some(borrowed) = clipboard_or_skip(
        ClipboardSnapshot::capture(),
        "capture clipboard before borrowed delivery",
    ) else {
        return;
    };

    let payload = "first line\nsecond line\n$() remains literal";
    let Some(()) = clipboard_or_skip(set_clipboard(payload), "stage multiline payload") else {
        return;
    };
    let Some(staged) = clipboard_or_skip(get_clipboard(), "read staged multiline payload") else {
        return;
    };
    assert_eq!(staged, payload);

    let Some(()) = clipboard_or_skip(borrowed.restore(), "restore borrowed clipboard") else {
        return;
    };
    let Some(restored) = clipboard_or_skip(get_clipboard(), "read restored clipboard") else {
        return;
    };
    assert_eq!(restored, "clipboard-owner-sentinel");
}

/// Stop-path contract: a plain hold / toggle session freezes `OrientDictation`,
/// Auto Paste on selects the clipboard route, Auto Paste off archives only,
/// and an assistive session never reaches the paste gun.
#[test]
fn stop_path_intent_follows_the_auto_paste_setting() {
    let dictation = delivery_intent_from_session(false, false, false);
    assert_eq!(dictation, DeliveryIntent::OrientDictation);

    let mut facts = overlay_insert_facts(true, false);
    facts.auto_paste_enabled = true;
    let on = resolve_delivery_route(dictation, facts);
    assert_eq!(on.route, DeliveryRoute::ClipboardPaste);
    assert_eq!(
        format_delivery_route_line(dictation, on, Some("Ghostty")),
        "delivery_route: intent=orient_dictation route=clipboard_paste reason=auto_paste target=Ghostty"
    );

    facts.auto_paste_enabled = false;
    let off = resolve_delivery_route(dictation, facts);
    assert_eq!(off.route, DeliveryRoute::ArchiveOnly);
    assert_eq!(off.reason, "auto_paste_disabled");

    facts.auto_paste_enabled = true;
    let assistive = delivery_intent_from_session(true, false, false);
    let agent = resolve_delivery_route(assistive, facts);
    assert_eq!(agent.route, DeliveryRoute::AgentComposer);
}
