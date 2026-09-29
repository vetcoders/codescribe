//! Main-thread AppKit regression. No capture, hotkeys, or external AX queries.
#[cfg(target_os = "macos")]
fn main() {
    use codescribe::os::hold_badge::{self, BadgeMode, HoldBadgeConfig};
    use codescribe::presentation::emitter::PresentationEmitter;
    use codescribe_core::pipeline::contracts::{
        EngineEvent, EventSink, UnadmittedAppleWord, UnadmittedAppleWordSource,
    };
    use core_foundation::base::TCFType;
    use core_foundation::runloop::{CFRunLoopRunInMode, kCFRunLoopDefaultMode};
    use core_foundation::string::CFString;
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::sync::Arc;
    type Id = *mut Object;

    fn drain() {
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.03, 0);
        }
    }
    unsafe fn visible_panel(app: Id) -> Id {
        unsafe {
            let windows: Id = msg_send![app, windows];
            let count: usize = msg_send![windows, count];
            for i in 0..count {
                let window: Id = msg_send![windows, objectAtIndex: i];
                let visible: bool = msg_send![window, isVisible];
                if visible {
                    return window;
                }
            }
            panic!("badge panel missing");
        }
    }
    unsafe fn text(panel: Id) -> String {
        unsafe {
            let content: Id = msg_send![panel, contentView];
            let children: Id = msg_send![content, subviews];
            let label: Id = msg_send![children, objectAtIndex: 1usize];
            let value: Id = msg_send![label, stringValue];
            CFString::wrap_under_get_rule(value.cast()).to_string()
        }
    }
    unsafe fn dot_color(panel: Id) -> [f64; 4] {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGColorGetComponents(color: *const std::ffi::c_void) -> *const f64;
            fn CGColorGetNumberOfComponents(color: *const std::ffi::c_void) -> usize;
        }
        unsafe {
            let content: Id = msg_send![panel, contentView];
            let children: Id = msg_send![content, subviews];
            let dot: Id = msg_send![children, objectAtIndex: 0usize];
            let layer: Id = msg_send![dot, layer];
            let color: *const std::ffi::c_void = msg_send![layer, backgroundColor];
            assert_eq!(CGColorGetNumberOfComponents(color), 4);
            std::slice::from_raw_parts(CGColorGetComponents(color), 4)
                .try_into()
                .unwrap()
        }
    }
    let config = |mode| HoldBadgeConfig {
        update_interval_ms: 60_000,
        ..HoldBadgeConfig::from_mode(mode)
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _runtime_guard = runtime.enter();
    let delivery = Arc::new(tokio::sync::Mutex::new(String::new()));
    unsafe {
        let pool: Id = msg_send![Class::get("NSAutoreleasePool").unwrap(), new];
        let app: Id = msg_send![Class::get("NSApplication").unwrap(), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: 2isize];
        hold_badge::show_hold_badge_with_config(config(BadgeMode::Hold));
        drain();
        let token = hold_badge::take_token();
        let emitter = PresentationEmitter::new(delivery.clone(), None, None).with_cursor_observer(
            Arc::new(move |projection| {
                hold_badge::update_transcript(token, &projection.text, projection.degraded);
            }),
        );
        emitter.on_capture_opened("native-preview", 1);
        let mirror = |revision, text: &str| EngineEvent::UnadmittedAppleWords {
            revision,
            words: text
                .split_whitespace()
                .map(|word| UnadmittedAppleWord {
                    text: word.into(),
                    sample_start: 0,
                    sample_end: 16_000,
                    source: UnadmittedAppleWordSource::OpenPartial {
                        rev: revision,
                        phrase_id: 1,
                    },
                })
                .collect(),
            closed_phrases: Default::default(),
        };
        emitter.on_event(&mirror(1, "last words from this take"));
        drain();
        assert_eq!(text(visible_panel(app)), "last words from this take");
        emitter.on_event(&EngineEvent::SpeechIntegrity {
            evidence: codescribe_core::pipeline::contracts::SpeechIntegrity {
                session_id: "native-preview".into(),
                capture_epoch: 1,
                sequence: 1,
                acoustic_speech_ms_since_text_advance: 2400,
                pending_occurrences: 1,
                phase: codescribe_core::pipeline::contracts::SpeechIntegrityPhase::Recovering,
            },
        });
        drain();
        let recording_panel = visible_panel(app);
        let warning_color = dot_color(recording_panel);
        assert_eq!(warning_color, [1.0, 0.62, 0.0, 1.0]);
        hold_badge::show_hold_badge_with_config(config(BadgeMode::Processing));
        drain();
        assert_eq!(
            text(visible_panel(app)),
            "last words from this take",
            "Processing must preserve the current take preview without another projection"
        );
        assert_eq!(
            visible_panel(app),
            recording_panel,
            "Mode transition reuses the take panel"
        );
        assert_eq!(
            dot_color(visible_panel(app)),
            warning_color,
            "Processing cannot clear unresolved speech evidence"
        );
        hold_badge::hide_hold_badge();
        drain();
        hold_badge::show_hold_badge_with_config(config(BadgeMode::Hold));
        drain();
        emitter.on_event(&mirror(2, "stale prior take"));
        drain();
        assert_eq!(
            text(visible_panel(app)),
            "",
            "A new take must reject old preview paint"
        );
        hold_badge::hide_hold_badge();
        drain();
        assert!(
            runtime.block_on(async { delivery.lock().await.is_empty() }),
            "Ephemeral preview must never become delivery text"
        );
        drop(emitter);
        let _: () = msg_send![pool, drain];
    }
    println!("PASS: native processing preview retention and stale-take rejection");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("SKIP: AppKit requires macOS");
}
