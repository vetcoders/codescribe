use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use dispatch::Queue;
use lazy_static::lazy_static;
use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use tracing::{info, warn};

use crate::config::Config;
use crate::ipc::{IpcCommand, IpcResponse};
use crate::ui::bootstrap::handlers::action_handler_class;
use crate::ui_helpers::{
    LabelConfig, NS_FLOATING_WINDOW_LEVEL, add_subview, button, button_set_action, color_rgba,
    color_white, create_label, set_text_field_string, window_close, window_show,
};

mod handlers;

// Type alias for Objective-C object pointers
type Id = *mut Object;

const BOOTSTRAP_WIDTH: f64 = 480.0;
const BOOTSTRAP_HEIGHT: f64 = 260.0;

const STEP_TEST_MIC: usize = 0;
const STEP_SHOW_OVERLAY: usize = 1;
const STEP_PRESS_HOTKEY: usize = 2;

#[derive(Default)]
struct BootstrapState {
    window: Option<usize>,
    step_labels: [Option<usize>; 3],
}

lazy_static! {
    static ref BOOTSTRAP_STATE: Mutex<BootstrapState> = Mutex::new(BootstrapState::default());
}

fn bootstrap_done_path() -> PathBuf {
    Config::config_dir().join("bootstrap_done")
}

pub fn should_show_bootstrap() -> bool {
    !bootstrap_done_path().exists()
}

fn mark_bootstrap_done() {
    let path = bootstrap_done_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, "done");
}

pub fn schedule_bootstrap() {
    if !should_show_bootstrap() {
        return;
    }

    thread::spawn(|| {
        thread::sleep(Duration::from_millis(800));
        show_bootstrap_overlay();
    });
}

pub fn show_bootstrap_overlay() {
    Queue::main().exec_async(|| {
        show_bootstrap_overlay_impl();
    });
}

fn show_bootstrap_overlay_impl() {
    unsafe {
        let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(window_ptr) = state.window {
            let window = window_ptr as Id;
            let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
            window_show(window);
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![window, makeKeyAndOrderFront: nil];
            return;
        }

        let ns_window = Class::get("NSWindow").unwrap();
        let ns_screen = Class::get("NSScreen").unwrap();
        let ns_visual = Class::get("NSVisualEffectView").unwrap();

        let main_screen: Id = msg_send![ns_screen, mainScreen];
        if main_screen.is_null() {
            warn!("No main screen available for bootstrap overlay");
            return;
        }
        let visible_frame: core_graphics::geometry::CGRect = msg_send![main_screen, visibleFrame];

        let x = visible_frame.origin.x + (visible_frame.size.width - BOOTSTRAP_WIDTH) / 2.0;
        let y = visible_frame.origin.y + (visible_frame.size.height - BOOTSTRAP_HEIGHT) / 2.0;

        let frame = core_graphics::geometry::CGRect {
            origin: core_graphics::geometry::CGPoint { x, y },
            size: core_graphics::geometry::CGSize {
                width: BOOTSTRAP_WIDTH,
                height: BOOTSTRAP_HEIGHT,
            },
        };

        let window: Id = msg_send![ns_window, alloc];
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView;
        let backing = NSBackingStoreType::Buffered;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style_mask
            backing: backing
            defer: false
        ];

        let _: () = msg_send![window, setOpaque: false];
        let clear_color = color_rgba(0.0, 0.0, 0.0, 0.0);
        let _: () = msg_send![window, setBackgroundColor: clear_color];
        let _: () = msg_send![window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let _: () = msg_send![window, setHasShadow: true];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![window, setCollectionBehavior: collection_behavior];

        let content_view: Id = msg_send![window, contentView];
        if content_view.is_null() {
            warn!("Failed to get content view for bootstrap overlay");
            return;
        }

        let blur_view: Id = msg_send![ns_visual, alloc];
        let blur_view: Id = msg_send![blur_view, initWithFrame: frame];
        let _: () = msg_send![blur_view, setMaterial: NSVisualEffectMaterial::HUDWindow];
        let _: () = msg_send![blur_view, setBlendingMode: NSVisualEffectBlendingMode::BehindWindow];
        let _: () = msg_send![blur_view, setState: NSVisualEffectState::Active];
        let _: () = msg_send![blur_view, setWantsLayer: true];
        let layer: Id = msg_send![blur_view, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: 16.0f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
        }
        add_subview(content_view, blur_view);

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let title_label = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, BOOTSTRAP_HEIGHT - 40.0),
                &core_graphics::geometry::CGSize::new(BOOTSTRAP_WIDTH - 40.0, 24.0),
            ),
            text: "Welcome to CodeScribe".to_string(),
            font_size: 15.0,
            bold: true,
            text_color: color_white(0.95),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, title_label);

        let subtitle_label = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, BOOTSTRAP_HEIGHT - 64.0),
                &core_graphics::geometry::CGSize::new(BOOTSTRAP_WIDTH - 40.0, 18.0),
            ),
            text: "3 quick steps (under 60s)".to_string(),
            font_size: 12.0,
            bold: false,
            text_color: color_white(0.7),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, subtitle_label);

        // Step 1: Test mic
        let step1_label = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, BOOTSTRAP_HEIGHT - 110.0),
                &core_graphics::geometry::CGSize::new(260.0, 20.0),
            ),
            text: "1) Test mic".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: color_white(0.9),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step1_label);

        let step1_status = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(300.0, BOOTSTRAP_HEIGHT - 110.0),
                &core_graphics::geometry::CGSize::new(80.0, 20.0),
            ),
            text: "pending".to_string(),
            font_size: 11.0,
            bold: false,
            text_color: color_white(0.6),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step1_status);

        let step1_btn = button(
            core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(380.0, BOOTSTRAP_HEIGHT - 114.0),
                &core_graphics::geometry::CGSize::new(80.0, 26.0),
            ),
            "Test",
        );
        button_set_action(step1_btn, action_handler, sel!(onTestMic:));
        add_subview(blur_view, step1_btn);

        // Step 2: Show overlay
        let step2_label = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, BOOTSTRAP_HEIGHT - 145.0),
                &core_graphics::geometry::CGSize::new(260.0, 20.0),
            ),
            text: "2) Show chat overlay".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: color_white(0.9),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step2_label);

        let step2_status = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(300.0, BOOTSTRAP_HEIGHT - 145.0),
                &core_graphics::geometry::CGSize::new(80.0, 20.0),
            ),
            text: "pending".to_string(),
            font_size: 11.0,
            bold: false,
            text_color: color_white(0.6),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step2_status);

        let step2_btn = button(
            core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(380.0, BOOTSTRAP_HEIGHT - 149.0),
                &core_graphics::geometry::CGSize::new(80.0, 26.0),
            ),
            "Show",
        );
        button_set_action(step2_btn, action_handler, sel!(onShowOverlay:));
        add_subview(blur_view, step2_btn);

        // Step 3: Press hotkey
        let step3_label = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, BOOTSTRAP_HEIGHT - 180.0),
                &core_graphics::geometry::CGSize::new(260.0, 20.0),
            ),
            text: "3) Press hotkey (Ctrl+Shift)".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: color_white(0.9),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step3_label);

        let step3_status = create_label(LabelConfig {
            frame: core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(300.0, BOOTSTRAP_HEIGHT - 180.0),
                &core_graphics::geometry::CGSize::new(80.0, 20.0),
            ),
            text: "pending".to_string(),
            font_size: 11.0,
            bold: false,
            text_color: color_white(0.6),
            background_color: None,
            selectable: false,
            editable: false,
        });
        add_subview(blur_view, step3_status);

        let step3_btn = button(
            core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(380.0, BOOTSTRAP_HEIGHT - 184.0),
                &core_graphics::geometry::CGSize::new(80.0, 26.0),
            ),
            "Done",
        );
        button_set_action(step3_btn, action_handler, sel!(onHotkeyDone:));
        add_subview(blur_view, step3_btn);

        // Footer buttons
        let finish_btn = button(
            core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(BOOTSTRAP_WIDTH - 110.0, 16.0),
                &core_graphics::geometry::CGSize::new(90.0, 28.0),
            ),
            "Finish",
        );
        button_set_action(finish_btn, action_handler, sel!(onFinish:));
        add_subview(blur_view, finish_btn);

        let skip_btn = button(
            core_graphics::geometry::CGRect::new(
                &core_graphics::geometry::CGPoint::new(20.0, 16.0),
                &core_graphics::geometry::CGSize::new(90.0, 28.0),
            ),
            "Skip",
        );
        button_set_action(skip_btn, action_handler, sel!(onFinish:));
        add_subview(blur_view, skip_btn);

        // Store state
        state.window = Some(window as usize);
        state.step_labels = [
            Some(step1_status as usize),
            Some(step2_status as usize),
            Some(step3_status as usize),
        ];

        window_show(window);
        let nil: *mut Object = std::ptr::null_mut();
        let _: () = msg_send![window, makeKeyAndOrderFront: nil];

        info!("Bootstrap overlay shown");
    }
}

pub(super) fn handle_test_mic() {
    update_step_status(STEP_TEST_MIC, "recording…");

    if let Err(e) = send_ipc(IpcCommand::StartRecording { assistive: false }) {
        warn!("Bootstrap test mic failed to start: {}", e);
        update_step_status(STEP_TEST_MIC, "failed");
        return;
    }

    thread::spawn(|| {
        thread::sleep(Duration::from_secs(3));
        let _ = send_ipc(IpcCommand::StopRecording);
        update_step_status(STEP_TEST_MIC, "done");
    });
}

pub(super) fn handle_show_overlay() {
    crate::show_voice_chat_overlay();
    crate::show_agent_tab();
    crate::voice_chat_ui::update_voice_chat_status("Listening...");
    update_step_status(STEP_SHOW_OVERLAY, "done");
}

pub(super) fn handle_hotkey_done() {
    update_step_status(STEP_PRESS_HOTKEY, "done");
}

pub(super) fn handle_finish() {
    mark_bootstrap_done();
    hide_bootstrap_overlay();
}

pub fn hide_bootstrap_overlay() {
    Queue::main().exec_async(|| unsafe {
        let mut state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(window_ptr) = state.window.take() {
            window_close(window_ptr as Id);
        }
        state.step_labels = [None, None, None];
    });
}

fn update_step_status(index: usize, text: &str) {
    let text = text.to_string();
    Queue::main().exec_async(move || unsafe {
        let state = BOOTSTRAP_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(label) = state.step_labels.get(index).and_then(|v| *v) {
            set_text_field_string(label as Id, &text);
        }
    });
}

fn send_ipc(cmd: IpcCommand) -> Result<IpcResponse, String> {
    let socket_path = crate::ipc::socket_path();
    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| format!("IPC connect failed: {e}"))?;
    let payload = serde_json::to_string(&cmd).map_err(|e| e.to_string())?;
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;

    serde_json::from_str::<IpcResponse>(&line).map_err(|e| e.to_string())
}
