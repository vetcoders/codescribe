mod steps;

use self::steps::{PermissionKind, TOTAL_STEPS, WizardStep, step_for_index};
use crate::config::{Config, HoldMods, ToggleTrigger, UserSettings, keychain};
use crate::os::hotkeys;
use crate::os::permissions::{self, PermissionStatus};
use crate::ui::shared::helpers::{
    LabelConfig, add_subview, button, button_set_action, color_clear, color_label,
    color_secondary_label, create_glass_effect_view_with, create_label, create_secure_text_input,
    ns_string, set_hidden, set_text_field_string, window_close, window_show,
};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use dispatch::Queue;
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};
use objc2_app_kit::{
    NSBackingStoreType, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

// Type alias for Objective-C object pointers
pub type Id = *mut Object;

const WINDOW_WIDTH: f64 = 720.0;
const WINDOW_HEIGHT: f64 = 540.0;
const FULL_DISK_STEP_INDEX: usize = 5;
const STATUS_NOT_DETERMINED: &str = "\u{25CB} Not Determined";
const STATUS_GRANTED: &str = "\u{25CF} Granted";
const STATUS_DENIED: &str = "\u{2715} Denied";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LanguageChoice {
    #[default]
    English,
    Polish,
}

impl LanguageChoice {
    fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Polish => "Polish",
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Polish => "pl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HotkeyModeChoice {
    HoldToTalk,
    Toggle,
    #[default]
    Both,
}

impl HotkeyModeChoice {
    fn label(self) -> &'static str {
        match self {
            Self::HoldToTalk => "Hold to Talk",
            Self::Toggle => "Toggle (Double-tap)",
            Self::Both => "Both",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PermissionUiStatus {
    #[default]
    NotDetermined,
    Granted,
    Denied,
}

#[derive(Clone, Copy, Default)]
struct UiRefs {
    sidebar_step_labels: [Option<usize>; TOTAL_STEPS],
    icon_label: Option<usize>,
    title_label: Option<usize>,
    description_label: Option<usize>,
    status_label: Option<usize>,
    instruction_label: Option<usize>,
    step_counter_label: Option<usize>,
    primary_button: Option<usize>,
    back_button: Option<usize>,
    skip_button: Option<usize>,
    language_view: Option<usize>,
    language_en_radio: Option<usize>,
    language_pl_radio: Option<usize>,
    api_view: Option<usize>,
    api_key_field: Option<usize>,
    api_hint_label: Option<usize>,
    hotkey_view: Option<usize>,
    hotkey_hold_radio: Option<usize>,
    hotkey_toggle_radio: Option<usize>,
    hotkey_both_radio: Option<usize>,
    summary_view: Option<usize>,
    summary_permission_labels: [Option<usize>; 5],
    summary_config_label: Option<usize>,
}

struct OnboardingState {
    window: Option<usize>,
    window_delegate: Option<usize>,
    action_handler: Option<usize>,
    step_index: usize,
    language: LanguageChoice,
    hotkey_mode: HotkeyModeChoice,
    requested_permissions: [bool; 5],
    permission_states: [PermissionUiStatus; 5],
    scheduled_auto_advance_step: Option<usize>,
    full_disk_polling: bool,
    api_key_configured: bool,
    ui: UiRefs,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            window: None,
            window_delegate: None,
            action_handler: None,
            step_index: 0,
            language: LanguageChoice::default(),
            hotkey_mode: HotkeyModeChoice::default(),
            requested_permissions: [false; 5],
            permission_states: [PermissionUiStatus::NotDetermined; 5],
            scheduled_auto_advance_step: None,
            full_disk_polling: false,
            api_key_configured: false,
            ui: UiRefs::default(),
        }
    }
}

static ONBOARDING_STATE: LazyLock<Mutex<OnboardingState>> =
    LazyLock::new(|| Mutex::new(OnboardingState::default()));

static ACTION_HANDLER_CLASS: OnceLock<&'static Class> = OnceLock::new();
static WINDOW_DELEGATE_CLASS: OnceLock<&'static Class> = OnceLock::new();

const PERMISSION_ORDER: [PermissionKind; 5] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
    PermissionKind::ScreenRecording,
    PermissionKind::FullDiskAccess,
];

fn onboarding_done_path() -> PathBuf {
    Config::config_dir().join("onboarding_done")
}

fn onboarding_progress_path() -> PathBuf {
    Config::config_dir().join("onboarding_progress")
}

fn onboarding_lock_path() -> PathBuf {
    Config::config_dir().join("onboarding_session.lock")
}

fn load_onboarding_progress() -> usize {
    let raw = fs::read_to_string(onboarding_progress_path()).ok();
    let step = raw
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    step.min(TOTAL_STEPS.saturating_sub(1))
}

fn save_onboarding_progress(step_index: usize) {
    let path = onboarding_progress_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, step_index.to_string());
}

fn clear_onboarding_progress() {
    let _ = fs::remove_file(onboarding_progress_path());
}

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }

    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn acquire_onboarding_lock() -> bool {
    let path = onboarding_lock_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let pid = std::process::id();

    let try_create = || -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let _ = write!(file, "{pid}");
        Ok(())
    };

    match try_create() {
        Ok(()) => return true,
        Err(e) if e.kind() != ErrorKind::AlreadyExists => {
            warn!("Onboarding: failed to acquire lock: {e}");
            return false;
        }
        Err(_) => {}
    }

    let existing_pid = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if existing_pid > 0 && existing_pid != pid && process_is_alive(existing_pid) {
        warn!(
            "Onboarding: lock is held by live process pid={existing_pid}, skipping duplicate wizard"
        );
        return false;
    }

    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            warn!("Onboarding: failed to remove stale lock: {e}");
            return false;
        }
    }

    match try_create() {
        Ok(()) => true,
        Err(e) => {
            warn!("Onboarding: failed to acquire lock: {e}");
            false
        }
    }
}

fn release_onboarding_lock() {
    let _ = fs::remove_file(onboarding_lock_path());
}

pub fn should_show_onboarding() -> bool {
    !onboarding_done_path().exists()
}

fn mark_onboarding_done() {
    clear_onboarding_progress();
    let path = onboarding_done_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, "done");
}

pub fn show_onboarding_wizard() {
    if !should_show_onboarding() {
        return;
    }
    if !acquire_onboarding_lock() {
        return;
    }

    if is_main_thread() {
        show_onboarding_wizard_impl();
        release_onboarding_lock();
    } else {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        Queue::main().exec_async(move || {
            show_onboarding_wizard_impl();
            let _ = tx.send(());
        });
        let _ = rx.recv();
        release_onboarding_lock();
    }
}

fn is_main_thread() -> bool {
    unsafe {
        let ns_thread = Class::get("NSThread").unwrap();
        msg_send![ns_thread, isMainThread]
    }
}

fn action_handler_class() -> &'static Class {
    ACTION_HANDLER_CLASS.get_or_init(|| unsafe {
        let superclass = Class::get("NSObject").expect("NSObject class missing");
        let mut decl = ClassDecl::new("CodeScribeOnboardingActionHandler", superclass)
            .expect("Failed to create onboarding action handler class");

        decl.add_method(
            sel!(onPrimaryAction:),
            on_primary_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onBackAction:),
            on_back_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onSkipAction:),
            on_skip_action as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onLanguageSelected:),
            on_language_selected as extern "C" fn(&Object, Sel, Id),
        );
        decl.add_method(
            sel!(onHotkeySelected:),
            on_hotkey_selected as extern "C" fn(&Object, Sel, Id),
        );

        decl.register()
    })
}

fn window_delegate_class() -> &'static Class {
    WINDOW_DELEGATE_CLASS.get_or_init(|| unsafe {
        let superclass = Class::get("NSObject").expect("NSObject class missing");
        let mut decl = ClassDecl::new("CodeScribeOnboardingWindowDelegate", superclass)
            .expect("Failed to create onboarding window delegate class");
        decl.add_method(
            sel!(windowWillClose:),
            on_window_will_close as extern "C" fn(&Object, Sel, Id),
        );
        decl.register()
    })
}

extern "C" fn on_primary_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_primary_action();
}

extern "C" fn on_back_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_back_action();
}

extern "C" fn on_skip_action(_this: &Object, _sel: Sel, _sender: Id) {
    handle_skip_action();
}

extern "C" fn on_language_selected(_this: &Object, _sel: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.language = if tag == 1 {
            LanguageChoice::Polish
        } else {
            LanguageChoice::English
        };
    }
    render_current_step();
}

extern "C" fn on_hotkey_selected(_this: &Object, _sel: Sel, sender: Id) {
    unsafe {
        let tag: isize = msg_send![sender, tag];
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_mode = match tag {
            1 => HotkeyModeChoice::Toggle,
            2 => HotkeyModeChoice::Both,
            _ => HotkeyModeChoice::HoldToTalk,
        };
    }
    render_current_step();
}

extern "C" fn on_window_will_close(_this: &Object, _sel: Sel, notification: Id) {
    let window_ptr = unsafe {
        if notification.is_null() {
            None
        } else {
            let window: Id = msg_send![notification, object];
            if window.is_null() {
                None
            } else {
                Some(window as usize)
            }
        }
    };

    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let delegate_ptr = state.window_delegate.take();
    let handler_ptr = state.action_handler.take();
    state.window = None;
    state.ui = UiRefs::default();
    state.full_disk_polling = false;
    state.scheduled_auto_advance_step = None;
    drop(state);

    unsafe {
        if let Some(ptr) = delegate_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = handler_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
        if let Some(ptr) = window_ptr {
            let _: () = msg_send![ptr as Id, release];
        }
    }

    stop_modal();
}

fn show_onboarding_wizard_impl() {
    unsafe {
        let existing = {
            let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.window
        };
        if let Some(window_ptr) = existing {
            let window = window_ptr as Id;
            let ns_window = Class::get("NSWindow").unwrap();
            let valid: bool = msg_send![window, isKindOfClass: ns_window];
            if valid {
                window_show(window);
                run_modal_for_window(window);
                return;
            }
        }

        let Some(screen_class) = Class::get("NSScreen") else {
            warn!("Onboarding: NSScreen class missing");
            return;
        };
        let screen: Id = msg_send![screen_class, mainScreen];
        if screen.is_null() {
            warn!("Onboarding: No main screen");
            return;
        }

        let visible: CGRect = msg_send![screen, visibleFrame];
        let origin_x = visible.origin.x + (visible.size.width - WINDOW_WIDTH) * 0.5;
        let origin_y = visible.origin.y + (visible.size.height - WINDOW_HEIGHT) * 0.5;
        let frame = CGRect::new(
            &CGPoint::new(origin_x, origin_y),
            &CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );

        let ns_window = Class::get("NSWindow").unwrap();
        let window: Id = msg_send![ns_window, alloc];
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView;
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style
            backing: NSBackingStoreType::Buffered
            defer: false
        ];

        let _: () = msg_send![window, setTitle: ns_string("Welcome to CodeScribe")];
        let _: () = msg_send![window, setTitleVisibility: 1_isize]; // NSWindowTitleHidden
        let _: () = msg_send![window, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setBackgroundColor: color_clear()];
        let _: () = msg_send![window, setReleasedWhenClosed: false];
        let _: () = msg_send![window, setMovableByWindowBackground: true];
        let _: () =
            msg_send![window, setCollectionBehavior: NSWindowCollectionBehavior::FullScreenNone];
        let size = CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT);
        let _: () = msg_send![window, setContentMinSize: size];
        let _: () = msg_send![window, setContentMaxSize: size];

        let close_btn: Id = msg_send![window, standardWindowButton: NSWindowButton::CloseButton];
        if !close_btn.is_null() {
            let _: () = msg_send![close_btn, setHidden: true];
            let _: () = msg_send![close_btn, setEnabled: false];
        }

        let mini_btn: Id =
            msg_send![window, standardWindowButton: NSWindowButton::MiniaturizeButton];
        if !mini_btn.is_null() {
            let _: () = msg_send![mini_btn, setHidden: true];
            let _: () = msg_send![mini_btn, setEnabled: false];
        }

        let zoom_btn: Id = msg_send![window, standardWindowButton: NSWindowButton::ZoomButton];
        if !zoom_btn.is_null() {
            let _: () = msg_send![zoom_btn, setHidden: true];
            let _: () = msg_send![zoom_btn, setEnabled: false];
        }

        let action_handler_class = action_handler_class();
        let action_handler: Id = msg_send![action_handler_class, new];

        let delegate_class = window_delegate_class();
        let window_delegate: Id = msg_send![delegate_class, new];
        let _: () = msg_send![window, setDelegate: window_delegate];

        let content_view: Id = msg_send![window, contentView];
        let content_bounds: CGRect = msg_send![content_view, bounds];
        let background = create_glass_effect_view_with(
            content_bounds,
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![background, setAutoresizingMask: 2_isize | 16_isize];
        add_subview(content_view, background);

        let ui = build_onboarding_ui(background, action_handler);

        let resume_step = load_onboarding_progress();

        {
            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            *state = OnboardingState {
                window: Some(window as usize),
                window_delegate: Some(window_delegate as usize),
                action_handler: Some(action_handler as usize),
                step_index: resume_step,
                language: initial_language_choice(),
                hotkey_mode: initial_hotkey_choice(),
                requested_permissions: [false; 5],
                permission_states: [PermissionUiStatus::NotDetermined; 5],
                scheduled_auto_advance_step: None,
                full_disk_polling: false,
                api_key_configured: keychain::load_key("LLM_API_KEY")
                    .map(|k| !k.trim().is_empty())
                    .unwrap_or(false),
                ui,
            };
            refresh_all_permission_states_locked(&mut state);
        }
        save_onboarding_progress(resume_step);

        render_current_step();
        window_show(window);
        run_modal_for_window(window);

        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.full_disk_polling = false;
        state.scheduled_auto_advance_step = None;
    }
}

fn build_onboarding_ui(root: Id, action_handler: Id) -> UiRefs {
    unsafe {
        let ns_view = Class::get("NSView").unwrap();
        let mut ui = UiRefs::default();

        const SIDEBAR_WIDTH: f64 = 204.0;
        let content_left = SIDEBAR_WIDTH + 22.0;
        let content_width = WINDOW_WIDTH - content_left - 22.0;
        let content_center = content_left + (content_width * 0.5);

        let sidebar_bg = create_glass_effect_view_with(
            CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(SIDEBAR_WIDTH, WINDOW_HEIGHT),
            ),
            NSVisualEffectMaterial::Sidebar,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            sidebar_bg,
            setAutoresizingMask: 16_isize | 2_isize // NSViewHeightSizable | NSViewWidthSizable
        ];
        add_subview(root, sidebar_bg);

        let content_bg = create_glass_effect_view_with(
            CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH, 0.0),
                &CGSize::new(WINDOW_WIDTH - SIDEBAR_WIDTH, WINDOW_HEIGHT),
            ),
            NSVisualEffectMaterial::HUDWindow,
            NSVisualEffectBlendingMode::BehindWindow,
            NSVisualEffectState::Active,
        );
        let _: () = msg_send![
            content_bg,
            setAutoresizingMask: 16_isize | 2_isize // NSViewHeightSizable | NSViewWidthSizable
        ];
        add_subview(root, content_bg);

        let sidebar: Id = msg_send![ns_view, alloc];
        let sidebar: Id = msg_send![
            sidebar,
            initWithFrame: CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(SIDEBAR_WIDTH, WINDOW_HEIGHT))
        ];
        let _: () = msg_send![sidebar, setAutoresizingMask: 16_isize | 2_isize]; // NSViewHeightSizable | NSViewWidthSizable
        add_subview(root, sidebar);

        let sidebar_title = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, 494.0),
                &CGSize::new(SIDEBAR_WIDTH - 28.0, 22.0),
            ),
            text: "Onboarding".to_string(),
            font_size: 13.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(sidebar_title, false, false);
        add_subview(sidebar, sidebar_title);

        let divider = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(SIDEBAR_WIDTH - 1.0, 0.0),
                &CGSize::new(1.0, WINDOW_HEIGHT),
            ),
            text: String::new(),
            background_color: Some(system_secondary_color()),
            ..Default::default()
        });
        add_subview(root, divider);

        let mut sidebar_step_labels: [Option<usize>; TOTAL_STEPS] = [None; TOTAL_STEPS];
        let mut y = 460.0;
        for slot in &mut sidebar_step_labels {
            let label = create_label(LabelConfig {
                frame: CGRect::new(
                    &CGPoint::new(18.0, y),
                    &CGSize::new(SIDEBAR_WIDTH - 28.0, 20.0),
                ),
                text: String::new(),
                font_size: 12.0,
                text_color: color_secondary_label(),
                ..Default::default()
            });
            configure_label(label, false, false);
            add_subview(sidebar, label);
            *slot = Some(label as usize);
            y -= 34.0;
        }
        ui.sidebar_step_labels = sidebar_step_labels;

        let icon_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_center - 50.0, 410.0),
                &CGSize::new(100.0, 34.0),
            ),
            text: String::new(),
            font_size: 24.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(icon_label, true, false);
        add_subview(root, icon_label);
        ui.icon_label = Some(icon_label as usize);

        let title_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left, 378.0),
                &CGSize::new(content_width, 34.0),
            ),
            text: String::new(),
            font_size: 24.0,
            bold: true,
            text_color: color_label(),
            ..Default::default()
        });
        configure_label(title_label, true, false);
        add_subview(root, title_label);
        ui.title_label = Some(title_label as usize);

        let description_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left + 8.0, 274.0),
                &CGSize::new(content_width - 16.0, 92.0),
            ),
            text: String::new(),
            font_size: 14.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(description_label, true, true);
        add_subview(root, description_label);
        ui.description_label = Some(description_label as usize);

        let status_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_center - 132.0, 244.0),
                &CGSize::new(264.0, 22.0),
            ),
            text: String::new(),
            font_size: 13.0,
            bold: true,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(status_label, true, false);
        add_subview(root, status_label);
        ui.status_label = Some(status_label as usize);

        let instruction_label = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(content_left + 8.0, 196.0),
                &CGSize::new(content_width - 16.0, 54.0),
            ),
            text: String::new(),
            font_size: 12.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(instruction_label, true, true);
        add_subview(root, instruction_label);
        ui.instruction_label = Some(instruction_label as usize);

        let language_view: Id = msg_send![ns_view, alloc];
        let language_view: Id = msg_send![
            language_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 160.0, 186.0),
                &CGSize::new(320.0, 88.0)
            )
        ];
        add_subview(root, language_view);
        ui.language_view = Some(language_view as usize);

        let language_en = create_radio_button(
            CGRect::new(&CGPoint::new(10.0, 52.0), &CGSize::new(300.0, 24.0)),
            "English",
            true,
        );
        let _: () = msg_send![language_en, setTag: 0_isize];
        button_set_action(language_en, action_handler, sel!(onLanguageSelected:));
        add_subview(language_view, language_en);
        ui.language_en_radio = Some(language_en as usize);

        let language_pl = create_radio_button(
            CGRect::new(&CGPoint::new(10.0, 22.0), &CGSize::new(300.0, 24.0)),
            "Polish",
            false,
        );
        let _: () = msg_send![language_pl, setTag: 1_isize];
        button_set_action(language_pl, action_handler, sel!(onLanguageSelected:));
        add_subview(language_view, language_pl);
        ui.language_pl_radio = Some(language_pl as usize);

        let api_view: Id = msg_send![ns_view, alloc];
        let api_view: Id = msg_send![
            api_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 190.0, 180.0),
                &CGSize::new(380.0, 92.0)
            )
        ];
        add_subview(root, api_view);
        ui.api_view = Some(api_view as usize);

        let api_key_field = create_secure_text_input(
            CGRect::new(&CGPoint::new(0.0, 46.0), &CGSize::new(380.0, 28.0)),
            "Enter your LLM API key",
        );
        add_subview(api_view, api_key_field);
        ui.api_key_field = Some(api_key_field as usize);

        let api_hint = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(0.0, 8.0), &CGSize::new(380.0, 30.0)),
            text: "Stored securely in macOS Keychain.".to_string(),
            font_size: 11.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(api_hint, true, true);
        add_subview(api_view, api_hint);
        ui.api_hint_label = Some(api_hint as usize);

        let hotkey_view: Id = msg_send![ns_view, alloc];
        let hotkey_view: Id = msg_send![
            hotkey_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 200.0, 164.0),
                &CGSize::new(400.0, 132.0)
            )
        ];
        add_subview(root, hotkey_view);
        ui.hotkey_view = Some(hotkey_view as usize);

        let hotkey_hold = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 90.0), &CGSize::new(390.0, 24.0)),
            "Hold to Talk (Ctrl+Option)",
            false,
        );
        let _: () = msg_send![hotkey_hold, setTag: 0_isize];
        button_set_action(hotkey_hold, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_hold);
        ui.hotkey_hold_radio = Some(hotkey_hold as usize);

        let hotkey_toggle = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 58.0), &CGSize::new(390.0, 24.0)),
            "Toggle (Double-tap Option)",
            false,
        );
        let _: () = msg_send![hotkey_toggle, setTag: 1_isize];
        button_set_action(hotkey_toggle, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_toggle);
        ui.hotkey_toggle_radio = Some(hotkey_toggle as usize);

        let hotkey_both = create_radio_button(
            CGRect::new(&CGPoint::new(0.0, 26.0), &CGSize::new(390.0, 24.0)),
            "Both",
            true,
        );
        let _: () = msg_send![hotkey_both, setTag: 2_isize];
        button_set_action(hotkey_both, action_handler, sel!(onHotkeySelected:));
        add_subview(hotkey_view, hotkey_both);
        ui.hotkey_both_radio = Some(hotkey_both as usize);

        let summary_view: Id = msg_send![ns_view, alloc];
        let summary_view: Id = msg_send![
            summary_view,
            initWithFrame: CGRect::new(
                &CGPoint::new(content_center - 188.0, 146.0),
                &CGSize::new(376.0, 196.0)
            )
        ];
        add_subview(root, summary_view);
        ui.summary_view = Some(summary_view as usize);

        let mut summary_labels: [Option<usize>; 5] = [None; 5];
        for (idx, permission) in PERMISSION_ORDER.iter().enumerate() {
            let y = 166.0 - (idx as f64 * 26.0);
            let label = create_label(LabelConfig {
                frame: CGRect::new(&CGPoint::new(0.0, y), &CGSize::new(360.0, 20.0)),
                text: permission.title().to_string(),
                font_size: 12.0,
                text_color: color_secondary_label(),
                ..Default::default()
            });
            configure_label(label, false, false);
            add_subview(summary_view, label);
            summary_labels[idx] = Some(label as usize);
        }
        ui.summary_permission_labels = summary_labels;

        let summary_config = create_label(LabelConfig {
            frame: CGRect::new(&CGPoint::new(0.0, 4.0), &CGSize::new(360.0, 50.0)),
            text: String::new(),
            font_size: 12.0,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(summary_config, false, true);
        add_subview(summary_view, summary_config);
        ui.summary_config_label = Some(summary_config as usize);

        let primary_w = 132.0;
        let skip_w = 106.0;
        let back_w = 90.0;
        let button_y = 16.0;
        let primary_x = WINDOW_WIDTH - 18.0 - primary_w;
        let skip_x = primary_x - 8.0 - skip_w;
        let back_x = skip_x - 8.0 - back_w;

        let primary_button = button(
            CGRect::new(
                &CGPoint::new(primary_x, button_y),
                &CGSize::new(primary_w, 32.0),
            ),
            "Continue",
        );
        button_set_action(primary_button, action_handler, sel!(onPrimaryAction:));
        add_subview(root, primary_button);
        ui.primary_button = Some(primary_button as usize);

        let back_button = button(
            CGRect::new(&CGPoint::new(back_x, button_y), &CGSize::new(back_w, 32.0)),
            "Back",
        );
        button_set_action(back_button, action_handler, sel!(onBackAction:));
        add_subview(root, back_button);
        ui.back_button = Some(back_button as usize);

        let skip_button = button(
            CGRect::new(&CGPoint::new(skip_x, button_y), &CGSize::new(skip_w, 32.0)),
            "Skip",
        );
        button_set_action(skip_button, action_handler, sel!(onSkipAction:));
        add_subview(root, skip_button);
        ui.skip_button = Some(skip_button as usize);

        let step_counter = create_label(LabelConfig {
            frame: CGRect::new(
                &CGPoint::new(18.0, 20.0),
                &CGSize::new(SIDEBAR_WIDTH - 28.0, 20.0),
            ),
            text: String::new(),
            font_size: 12.0,
            bold: true,
            text_color: color_secondary_label(),
            ..Default::default()
        });
        configure_label(step_counter, false, false);
        add_subview(sidebar, step_counter);
        ui.step_counter_label = Some(step_counter as usize);

        ui
    }
}

fn render_current_step() {
    let (step_index, step, language, hotkey_mode, api_key_configured, permissions, ui) = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let step = step_for_index(state.step_index);
        match step {
            WizardStep::Permission(kind) => {
                let idx = kind.index();
                let requested = state.requested_permissions[idx];
                state.permission_states[idx] = check_permission_state(kind, requested);
            }
            WizardStep::Done => {
                refresh_all_permission_states_locked(&mut state);
            }
            _ => {}
        }

        (
            state.step_index,
            step,
            state.language,
            state.hotkey_mode,
            state.api_key_configured,
            state.permission_states,
            state.ui,
        )
    };

    set_text_if_present(
        ui.step_counter_label,
        &format!("Step {} of {}", step_index + 1, TOTAL_STEPS),
    );

    set_hidden_if_present(ui.status_label, true);
    set_hidden_if_present(ui.instruction_label, true);
    set_hidden_if_present(ui.language_view, true);
    set_hidden_if_present(ui.api_view, true);
    set_hidden_if_present(ui.hotkey_view, true);
    set_hidden_if_present(ui.summary_view, true);
    set_hidden_if_present(ui.skip_button, true);

    set_hidden_if_present(ui.back_button, step_index == 0);

    sync_language_radios(ui, language);
    sync_hotkey_radios(ui, hotkey_mode);
    update_sidebar_step_labels(ui, step_index, permissions);

    match step {
        WizardStep::Welcome => {
            set_text_if_present(ui.icon_label, "WELCOME");
            set_text_if_present(ui.title_label, "Welcome to CodeScribe");
            set_text_if_present(
                ui.description_label,
                "We will guide you through permissions and setup so CodeScribe works perfectly from first launch.",
            );
            set_button_title_if_present(ui.primary_button, "Get Started");
        }
        WizardStep::Permission(kind) => {
            let status = permissions[kind.index()];
            set_text_if_present(ui.icon_label, kind.icon());
            set_text_if_present(ui.title_label, kind.title());
            set_text_if_present(ui.description_label, kind.reason());

            set_hidden_if_present(ui.status_label, false);
            set_text_if_present(ui.status_label, permission_status_text(status));
            set_label_color_if_present(ui.status_label, permission_status_color(status));

            if status == PermissionUiStatus::Granted {
                set_button_title_if_present(ui.primary_button, "Continue");
                maybe_schedule_auto_advance(step_index);
            } else if kind == PermissionKind::FullDiskAccess {
                set_button_title_if_present(ui.primary_button, "Open Settings");
                set_hidden_if_present(ui.skip_button, false);
                set_button_title_if_present(
                    ui.skip_button,
                    if status == PermissionUiStatus::Denied {
                        "Continue Anyway"
                    } else {
                        "Skip"
                    },
                );
                set_hidden_if_present(ui.instruction_label, false);
                set_text_if_present(
                    ui.instruction_label,
                    "Find CodeScribe in the list and toggle it ON. This step is optional.",
                );
            } else {
                set_button_title_if_present(
                    ui.primary_button,
                    if status == PermissionUiStatus::Denied {
                        "Try Again"
                    } else {
                        "Grant Access"
                    },
                );
                if status == PermissionUiStatus::Denied {
                    set_hidden_if_present(ui.instruction_label, false);
                    set_text_if_present(
                        ui.instruction_label,
                        "This permission is required to continue onboarding. If status does not refresh after enabling it in System Settings, restart CodeScribe.",
                    );
                }
            }
        }
        WizardStep::Language => {
            set_text_if_present(ui.icon_label, "LANG");
            set_text_if_present(ui.title_label, "Choose Language");
            set_text_if_present(
                ui.description_label,
                "Select the default transcription language. You can change it later in Settings.",
            );
            set_hidden_if_present(ui.language_view, false);
            set_button_title_if_present(ui.primary_button, "Continue");
        }
        WizardStep::ApiKey => {
            set_text_if_present(ui.icon_label, "API");
            set_text_if_present(ui.title_label, "Add API Key (Optional)");
            set_text_if_present(
                ui.description_label,
                "Use your LLM API key for AI formatting and assistant features.",
            );
            set_hidden_if_present(ui.api_view, false);
            set_button_title_if_present(ui.primary_button, "Save & Continue");
            set_hidden_if_present(ui.skip_button, false);
            set_button_title_if_present(ui.skip_button, "Skip (Offline)");
        }
        WizardStep::HotkeyMode => {
            set_text_if_present(ui.icon_label, "HOTKEY");
            set_text_if_present(ui.title_label, "Choose Hotkey Mode");
            set_text_if_present(
                ui.description_label,
                "Pick how you want to start and stop recording.",
            );
            set_hidden_if_present(ui.hotkey_view, false);
            set_button_title_if_present(ui.primary_button, "Continue");
        }
        WizardStep::Done => {
            set_text_if_present(ui.icon_label, "DONE");
            set_text_if_present(ui.title_label, "You're All Set");
            set_text_if_present(
                ui.description_label,
                "Review your setup. You can always adjust these settings later.",
            );
            set_hidden_if_present(ui.summary_view, false);
            update_summary_view(ui, permissions, language, api_key_configured, hotkey_mode);
            set_button_title_if_present(ui.primary_button, "Start CodeScribe");
        }
    }
}

fn handle_primary_action() {
    let step = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        step_for_index(state.step_index)
    };

    match step {
        WizardStep::Welcome => advance_step(),
        WizardStep::Permission(kind) => handle_permission_primary(kind),
        WizardStep::Language => {
            save_language_choice();
            advance_step();
        }
        WizardStep::ApiKey => {
            if persist_api_key_from_field() {
                advance_step();
            }
        }
        WizardStep::HotkeyMode => {
            save_hotkey_mode();
            advance_step();
        }
        WizardStep::Done => finish_onboarding(true),
    }
}

fn handle_back_action() {
    retreat_step();
}

fn handle_skip_action() {
    let step = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        step_for_index(state.step_index)
    };

    match step {
        WizardStep::Permission(PermissionKind::FullDiskAccess) => {
            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.full_disk_polling = false;
            drop(state);
            advance_step();
        }
        WizardStep::ApiKey => {
            mark_api_key_skipped();
            advance_step();
        }
        _ => {}
    }
}

fn handle_permission_primary(kind: PermissionKind) {
    let idx = kind.index();
    let step_to_persist;

    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.permission_states[idx] == PermissionUiStatus::Granted {
            drop(state);
            advance_step();
            return;
        }

        state.requested_permissions[idx] = true;
        step_to_persist = state.step_index;
    }

    // Persist checkpoint before asking TCC in case macOS forces an app restart.
    save_onboarding_progress(step_to_persist);

    let _ = request_permission(kind);

    if kind == PermissionKind::FullDiskAccess {
        start_full_disk_polling();
    }

    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        let requested = state.requested_permissions[idx];
        state.permission_states[idx] = check_permission_state(kind, requested);
    }

    render_current_step();
}

fn advance_step() {
    let mut should_render = false;
    let mut new_step = None;
    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.step_index + 1 < TOTAL_STEPS {
            state.step_index += 1;
            state.scheduled_auto_advance_step = None;
            if state.step_index != FULL_DISK_STEP_INDEX {
                state.full_disk_polling = false;
            }
            new_step = Some(state.step_index);
            should_render = true;
        }
    }
    if let Some(step) = new_step {
        save_onboarding_progress(step);
    }

    if should_render {
        render_current_step();
    }
}

fn retreat_step() {
    let mut should_render = false;
    let mut new_step = None;
    {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.step_index > 0 {
            state.step_index -= 1;
            state.scheduled_auto_advance_step = None;
            if state.step_index != FULL_DISK_STEP_INDEX {
                state.full_disk_polling = false;
            }
            new_step = Some(state.step_index);
            should_render = true;
        }
    }
    if let Some(step) = new_step {
        save_onboarding_progress(step);
    }

    if should_render {
        render_current_step();
    }
}

fn maybe_schedule_auto_advance(step_index: usize) {
    let should_schedule = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.scheduled_auto_advance_step == Some(step_index) {
            false
        } else {
            state.scheduled_auto_advance_step = Some(step_index);
            true
        }
    };

    if !should_schedule {
        return;
    }

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(800));
        Queue::main().exec_async(move || {
            let mut should_advance = false;

            {
                let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                if state.step_index == step_index
                    && let WizardStep::Permission(kind) = step_for_index(step_index)
                {
                    let idx = kind.index();
                    let requested = state.requested_permissions[idx];
                    let status = check_permission_state(kind, requested);
                    state.permission_states[idx] = status;
                    if status == PermissionUiStatus::Granted {
                        should_advance = true;
                    }
                }
                state.scheduled_auto_advance_step = None;
            }

            if should_advance {
                advance_step();
            } else {
                render_current_step();
            }
        });
    });
}

fn start_full_disk_polling() {
    let should_start = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.full_disk_polling {
            false
        } else {
            state.full_disk_polling = true;
            true
        }
    };

    if !should_start {
        return;
    }

    thread::spawn(|| {
        loop {
            thread::sleep(Duration::from_secs(2));

            let keep_running = {
                let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.full_disk_polling
            };

            if !keep_running {
                break;
            }

            Queue::main().exec_async(|| {
                let mut granted = false;
                let mut should_render = false;

                {
                    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if step_for_index(state.step_index)
                        == WizardStep::Permission(PermissionKind::FullDiskAccess)
                    {
                        let idx = PermissionKind::FullDiskAccess.index();
                        state.permission_states[idx] =
                            check_permission_state(PermissionKind::FullDiskAccess, true);
                        granted = state.permission_states[idx] == PermissionUiStatus::Granted;
                        if granted {
                            state.full_disk_polling = false;
                        }
                        should_render = true;
                    } else {
                        state.full_disk_polling = false;
                    }
                }

                if should_render {
                    render_current_step();
                }
                if granted {
                    maybe_schedule_auto_advance(FULL_DISK_STEP_INDEX);
                }
            });
        }
    });
}

fn save_language_choice() {
    let language = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.language
    };

    let mut settings = UserSettings::load();
    settings.whisper_language = Some(language.value().to_string());
    if let Err(e) = settings.save() {
        warn!(
            "Onboarding: failed to persist language {}: {e}",
            language.value()
        );
    }

    unsafe { std::env::set_var("WHISPER_LANGUAGE", language.value()) };
    info!("Onboarding: language set to {}", language.value());
}

fn persist_api_key_from_field() -> bool {
    let key = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state
            .ui
            .api_key_field
            .map(|ptr| get_text_field_string(ptr as Id))
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if key.is_empty() {
        mark_api_key_skipped();
        return true;
    }

    match keychain::save_key("LLM_API_KEY", &key) {
        Ok(()) => {
            unsafe { std::env::set_var("LLM_API_KEY", &key) };
            let mut settings = UserSettings::load();
            settings.ai_formatting_enabled = Some(true);
            if let Err(e) = settings.save() {
                warn!("Onboarding: failed to persist AI formatting setting: {e}");
            }

            unsafe { std::env::set_var("AI_FORMATTING_ENABLED", "1") };

            let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.api_key_configured = true;

            if let Some(label_ptr) = state.ui.api_hint_label {
                unsafe {
                    set_text_field_string(label_ptr as Id, "API key saved to Keychain.");
                    let green = system_green_color();
                    let _: () = msg_send![label_ptr as Id, setTextColor: green];
                }
            }
            true
        }
        Err(e) => {
            warn!("Onboarding: failed to save API key: {e}");
            let state = ONBOARDING_STATE
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(label_ptr) = state.ui.api_hint_label {
                unsafe {
                    set_text_field_string(label_ptr as Id, "Failed to save key. Please try again.");
                    let red = system_red_color();
                    let _: () = msg_send![label_ptr as Id, setTextColor: red];
                }
            }
            false
        }
    }
}

fn mark_api_key_skipped() {
    let mut settings = UserSettings::load();
    settings.ai_formatting_enabled = Some(false);
    if let Err(e) = settings.save() {
        warn!("Onboarding: failed to persist AI formatting disabled state: {e}");
    }

    unsafe { std::env::set_var("AI_FORMATTING_ENABLED", "0") };

    let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.api_key_configured = false;
}

fn save_hotkey_mode() {
    let mode = {
        let state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_mode
    };

    let (
        hold_mods_raw,
        toggle_trigger_raw,
        double_tap_left,
        double_tap_right,
        hold_mods_runtime,
        toggle_trigger_runtime,
    ) = match mode {
        HotkeyModeChoice::HoldToTalk => (
            "ctrl_alt",
            "none",
            false,
            false,
            HoldMods::CtrlAlt,
            ToggleTrigger::None,
        ),
        HotkeyModeChoice::Toggle => (
            "none",
            "double_option",
            true,
            false,
            HoldMods::None,
            ToggleTrigger::DoubleOption,
        ),
        HotkeyModeChoice::Both => (
            "ctrl_alt",
            "double_option",
            true,
            true,
            HoldMods::CtrlAlt,
            ToggleTrigger::DoubleOption,
        ),
    };

    let mut settings = UserSettings::load();
    settings.hold_mods = Some(hold_mods_raw.to_string());
    settings.toggle_trigger = Some(toggle_trigger_raw.to_string());
    settings.double_tap_left = Some(double_tap_left);
    settings.double_tap_right = Some(double_tap_right);
    if let Err(e) = settings.save() {
        warn!(
            "Onboarding: failed to persist hotkey mode {}: {e}",
            mode.label()
        );
    }

    hotkeys::set_hold_mods(hold_mods_runtime);
    hotkeys::set_toggle_trigger(toggle_trigger_runtime);
    unsafe {
        std::env::set_var("HOLD_MODS", hold_mods_raw);
        std::env::set_var("TOGGLE_TRIGGER", toggle_trigger_raw);
        std::env::set_var(
            "HOTKEY_DOUBLE_TAP_LEFT",
            if double_tap_left { "1" } else { "0" },
        );
        std::env::set_var(
            "HOTKEY_DOUBLE_TAP_RIGHT",
            if double_tap_right { "1" } else { "0" },
        );
    }

    info!("Onboarding: hotkey mode set to {}", mode.label());
}

fn finish_onboarding(completed: bool) {
    if completed {
        mark_onboarding_done();
    }

    let window_ptr = {
        let mut state = ONBOARDING_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.full_disk_polling = false;
        state.scheduled_auto_advance_step = None;
        state.window.take()
    };

    stop_modal();

    if let Some(ptr) = window_ptr {
        unsafe { window_close(ptr as Id) };
    }
}

fn stop_modal() {
    unsafe {
        let ns_app = Class::get("NSApplication").unwrap();
        let app: Id = msg_send![ns_app, sharedApplication];
        let _: () = msg_send![app, stopModal];
    }
}

fn run_modal_for_window(window: Id) {
    unsafe {
        let ns_app = Class::get("NSApplication").unwrap();
        let app: Id = msg_send![ns_app, sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let _: isize = msg_send![app, runModalForWindow: window];
    }
}

fn initial_language_choice() -> LanguageChoice {
    let settings = UserSettings::load();
    match settings.whisper_language.as_deref() {
        Some("pl") => LanguageChoice::Polish,
        _ => LanguageChoice::English,
    }
}

fn initial_hotkey_choice() -> HotkeyModeChoice {
    let settings = UserSettings::load();

    let hold_raw = settings
        .hold_mods
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();

    let toggle_raw = settings
        .toggle_trigger
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();

    let hold_enabled = !hold_raw.is_empty() && hold_raw != "none";
    let toggle_enabled = if !toggle_raw.is_empty() {
        toggle_raw != "none"
    } else {
        settings.double_tap_left.unwrap_or(false) || settings.double_tap_right.unwrap_or(false)
    };

    match (hold_enabled, toggle_enabled) {
        (true, true) => HotkeyModeChoice::Both,
        (true, false) => HotkeyModeChoice::HoldToTalk,
        (false, true) => HotkeyModeChoice::Toggle,
        (false, false) => HotkeyModeChoice::Both,
    }
}

fn refresh_all_permission_states_locked(state: &mut OnboardingState) {
    for kind in PERMISSION_ORDER {
        let idx = kind.index();
        state.permission_states[idx] =
            check_permission_state(kind, state.requested_permissions[idx]);
    }
}

fn check_permission_state(kind: PermissionKind, requested: bool) -> PermissionUiStatus {
    match kind {
        PermissionKind::Microphone => {
            map_permission_status(permissions::check_microphone(), requested)
        }
        PermissionKind::Accessibility => {
            map_permission_status(permissions::check_accessibility(), requested)
        }
        PermissionKind::InputMonitoring => {
            map_permission_status(permissions::check_input_monitoring(), requested)
        }
        PermissionKind::ScreenRecording => {
            map_permission_status(permissions::check_screen_recording(), requested)
        }
        PermissionKind::FullDiskAccess => {
            map_permission_status(permissions::check_full_disk_access(), requested)
        }
    }
}

fn map_permission_status(status: PermissionStatus, requested: bool) -> PermissionUiStatus {
    match status {
        PermissionStatus::Granted => PermissionUiStatus::Granted,
        PermissionStatus::Denied => PermissionUiStatus::Denied,
        PermissionStatus::NotDetermined => {
            if requested {
                PermissionUiStatus::Denied
            } else {
                PermissionUiStatus::NotDetermined
            }
        }
    }
}

fn request_permission(kind: PermissionKind) -> bool {
    match kind {
        PermissionKind::Microphone => {
            let result = permissions::request_microphone();
            if !result {
                permissions::open_privacy_settings("Privacy_Microphone");
            }
            result
        }
        PermissionKind::Accessibility => permissions::request_accessibility(),
        PermissionKind::InputMonitoring => permissions::request_input_monitoring(),
        PermissionKind::ScreenRecording => {
            let result = permissions::request_screen_recording();
            if !result {
                permissions::open_privacy_settings("Privacy_ScreenCapture");
            }
            result
        }
        PermissionKind::FullDiskAccess => permissions::request_full_disk_access(),
    }
}

fn configure_label(label: Id, centered: bool, multiline: bool) {
    unsafe {
        // NSTextAlignment: 0=left, 1=center, 2=right
        let align = if centered { 1_isize } else { 0_isize };
        let _: () = msg_send![label, setAlignment: align];

        if multiline {
            let _: () = msg_send![label, setUsesSingleLineMode: false];
            let _: () = msg_send![label, setLineBreakMode: 0_isize];
            let cell: Id = msg_send![label, cell];
            if !cell.is_null() {
                let _: () = msg_send![cell, setWraps: true];
                let _: () = msg_send![cell, setScrollable: false];
                let _: () = msg_send![cell, setLineBreakMode: 0_isize];
            }
        }
    }
}

fn create_radio_button(frame: CGRect, title: &str, selected: bool) -> Id {
    unsafe {
        let ns_button = Class::get("NSButton").unwrap();
        let button: Id = msg_send![ns_button, alloc];
        let button: Id = msg_send![button, initWithFrame: frame];
        let _: () = msg_send![button, setButtonType: 4_isize]; // NSRadioButton
        let _: () = msg_send![button, setTitle: ns_string(title)];
        let _: () = msg_send![button, setState: if selected { 1_isize } else { 0_isize }];
        button
    }
}

fn set_text_if_present(ptr: Option<usize>, text: &str) {
    unsafe {
        if let Some(value) = ptr {
            set_text_field_string(value as Id, text);
        }
    }
}

fn set_button_title_if_present(ptr: Option<usize>, title: &str) {
    unsafe {
        if let Some(value) = ptr {
            let _: () = msg_send![value as Id, setTitle: ns_string(title)];
        }
    }
}

fn set_hidden_if_present(ptr: Option<usize>, hidden: bool) {
    unsafe {
        if let Some(value) = ptr {
            set_hidden(value as Id, hidden);
        }
    }
}

fn set_label_color_if_present(ptr: Option<usize>, color: Id) {
    unsafe {
        if let Some(value) = ptr {
            let _: () = msg_send![value as Id, setTextColor: color];
        }
    }
}

fn sync_language_radios(ui: UiRefs, language: LanguageChoice) {
    unsafe {
        if let Some(en) = ui.language_en_radio {
            let _: () = msg_send![en as Id, setState: if language == LanguageChoice::English { 1_isize } else { 0_isize }];
        }
        if let Some(pl) = ui.language_pl_radio {
            let _: () = msg_send![pl as Id, setState: if language == LanguageChoice::Polish { 1_isize } else { 0_isize }];
        }
    }
}

fn sync_hotkey_radios(ui: UiRefs, mode: HotkeyModeChoice) {
    unsafe {
        if let Some(hold) = ui.hotkey_hold_radio {
            let _: () = msg_send![hold as Id, setState: if mode == HotkeyModeChoice::HoldToTalk { 1_isize } else { 0_isize }];
        }
        if let Some(toggle) = ui.hotkey_toggle_radio {
            let _: () = msg_send![toggle as Id, setState: if mode == HotkeyModeChoice::Toggle { 1_isize } else { 0_isize }];
        }
        if let Some(both) = ui.hotkey_both_radio {
            let _: () = msg_send![both as Id, setState: if mode == HotkeyModeChoice::Both { 1_isize } else { 0_isize }];
        }
    }
}

fn sidebar_step_title(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Welcome => "Welcome",
        WizardStep::Permission(PermissionKind::Microphone) => "Microphone",
        WizardStep::Permission(PermissionKind::Accessibility) => "Accessibility",
        WizardStep::Permission(PermissionKind::InputMonitoring) => "Input Monitoring",
        WizardStep::Permission(PermissionKind::ScreenRecording) => "Screen Recording",
        WizardStep::Permission(PermissionKind::FullDiskAccess) => "Full Disk Access",
        WizardStep::Language => "Language",
        WizardStep::ApiKey => "API Key",
        WizardStep::HotkeyMode => "Hotkeys",
        WizardStep::Done => "Finish",
    }
}

fn update_sidebar_step_labels(
    ui: UiRefs,
    current_step_index: usize,
    permissions: [PermissionUiStatus; 5],
) {
    for idx in 0..TOTAL_STEPS {
        let step = step_for_index(idx);
        let (marker, color) = if idx == current_step_index {
            if let WizardStep::Permission(kind) = step {
                let status = permissions[kind.index()];
                if status == PermissionUiStatus::Denied {
                    ("\u{2715}", system_red_color())
                } else {
                    ("\u{25CF}", color_label())
                }
            } else {
                ("\u{25CF}", color_label())
            }
        } else if idx < current_step_index {
            if let WizardStep::Permission(PermissionKind::FullDiskAccess) = step {
                if permissions[PermissionKind::FullDiskAccess.index()]
                    != PermissionUiStatus::Granted
                {
                    ("\u{2013}", system_secondary_color())
                } else {
                    ("\u{2713}", system_green_color())
                }
            } else {
                ("\u{2713}", system_green_color())
            }
        } else {
            ("\u{25CB}", system_secondary_color())
        };

        let text = format!("{marker} {}", sidebar_step_title(step));
        set_text_if_present(ui.sidebar_step_labels[idx], &text);
        set_label_color_if_present(ui.sidebar_step_labels[idx], color);
    }
}

fn update_summary_view(
    ui: UiRefs,
    statuses: [PermissionUiStatus; 5],
    language: LanguageChoice,
    api_key_configured: bool,
    hotkey_mode: HotkeyModeChoice,
) {
    for kind in PERMISSION_ORDER {
        let idx = kind.index();
        let text = if statuses[idx] == PermissionUiStatus::Granted {
            format!("\u{2713} {}", kind.title())
        } else {
            format!("\u{2715} {}", kind.title())
        };
        set_text_if_present(ui.summary_permission_labels[idx], &text);

        let color = if statuses[idx] == PermissionUiStatus::Granted {
            system_green_color()
        } else {
            system_red_color()
        };
        set_label_color_if_present(ui.summary_permission_labels[idx], color);
    }

    let api_status = if api_key_configured {
        "Configured"
    } else {
        "Skipped (Offline mode)"
    };

    set_text_if_present(
        ui.summary_config_label,
        &format!(
            "Language: {}\nAPI key: {}\nHotkey mode: {}",
            language.label(),
            api_status,
            hotkey_mode.label()
        ),
    );
}

fn permission_status_text(status: PermissionUiStatus) -> &'static str {
    match status {
        PermissionUiStatus::NotDetermined => STATUS_NOT_DETERMINED,
        PermissionUiStatus::Granted => STATUS_GRANTED,
        PermissionUiStatus::Denied => STATUS_DENIED,
    }
}

fn permission_status_color(status: PermissionUiStatus) -> Id {
    match status {
        PermissionUiStatus::NotDetermined => system_secondary_color(),
        PermissionUiStatus::Granted => system_green_color(),
        PermissionUiStatus::Denied => system_red_color(),
    }
}

fn system_green_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, systemGreenColor]
    }
}

fn system_red_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, systemRedColor]
    }
}

fn system_secondary_color() -> Id {
    unsafe {
        let ns_color = Class::get("NSColor").unwrap();
        msg_send![ns_color, secondaryLabelColor]
    }
}

fn get_text_field_string(field: Id) -> String {
    unsafe {
        let value: Id = msg_send![field, stringValue];
        let c_str: *const std::ffi::c_char = msg_send![value, UTF8String];
        if c_str.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(c_str)
            .to_string_lossy()
            .to_string()
    }
}
