//! System tray icon and menu for CodeScribe
//!
//! Provides visual status feedback and menu controls via macOS menu bar icon.
//! Uses tao event loop for proper macOS integration.
//!
//! ## Unwired Menu Handlers (TODO)
//!
//! The following menu events are sent but NOT yet handled in main.rs:
//! - `SetHoldMods` - Change hold modifier keys (needs hotkey reconfiguration)
//! - `ToggleHoldExclusive` - Toggle exclusive mode (needs hotkey reconfiguration)
//! - `SetToggleTrigger` - Change toggle trigger (needs hotkey reconfiguration)
//! - `ToggleStatusGlyph` - Show/hide status glyph (needs tray icon update)
//! - `RefreshTrayIcon` - Force refresh icon (needs tray icon update)
//! - `ToggleStartSound` - Enable/disable beep (needs config update)
//! - `SetSoundType` - Change sound type (needs config update)
//! - `SetVolume` - Set volume level (needs dialog/slider implementation)
//! - `CheckPermissions` - Refresh permission status (handled locally in tray.rs)
//!
//! Note: OpenAccessibilitySettings and OpenMicrophoneSettings ARE handled
//! directly in tray.rs handle_menu_event (they open System Settings).
#![allow(dead_code)]

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use image::{imageops::FilterType, GenericImageView};
use muda::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tracing::{debug, info};
use tray_icon::{menu::MenuEvent, Icon, TrayIconBuilder};

// Re-export config enums for menu use (single source of truth)
pub use crate::config::{HoldMods, Language, ToggleTrigger};

/// Embedded CodeScribe logo icon (resized for menu bar)
/// Place icon.png in codescribe-rs/assets/ directory
const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.png");

/// Menu bar icon size (44x44 for Retina, 22x22 logical)
const ICON_SIZE: u32 = 44;

/// Global flag for status glyph visibility
static SHOW_STATUS_GLYPH: AtomicBool = AtomicBool::new(true);

/// Set whether the status glyph (colored dot) is visible on the icon
pub fn set_status_glyph_enabled(enabled: bool) {
    SHOW_STATUS_GLYPH.store(enabled, Ordering::SeqCst);
    debug!(
        "Status glyph {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

/// Get whether the status glyph is currently enabled
pub fn is_status_glyph_enabled() -> bool {
    SHOW_STATUS_GLYPH.load(Ordering::SeqCst)
}

/// Model menu items for dynamic updates
struct ModelMenuItems {
    small: CheckMenuItem,
    medium: CheckMenuItem,
    large_v3: CheckMenuItem,
    large_v3_turbo: CheckMenuItem,
    label: MenuItem,
}

// Thread-local storage for model menu items (CheckMenuItem contains Rc, not Send/Sync)
// Updates are done via MODEL_UPDATE_CHANNEL from other threads
thread_local! {
    static MODEL_MENU_ITEMS: RefCell<Option<ModelMenuItems>> = const { RefCell::new(None) };
}

/// Channel for model selection updates from async tasks
static MODEL_UPDATE_CHANNEL: OnceLock<Sender<String>> = OnceLock::new();

/// Update the model selection in the menu
///
/// Variant should be one of: "small", "medium", "large-v3", "large-v3-turbo"
/// Thread-safe: can be called from any thread (sends via channel to main thread)
pub fn update_model_selection(variant: &str) {
    if let Some(sender) = MODEL_UPDATE_CHANNEL.get() {
        if let Err(e) = sender.send(variant.to_string()) {
            debug!("Failed to send model update: {}", e);
        }
    } else {
        debug!("Model update channel not initialized");
    }
}

/// Actually update the model menu items (must be called on main thread)
fn apply_model_selection(variant: &str) {
    MODEL_MENU_ITEMS.with(|items_cell| {
        if let Some(items) = items_cell.borrow().as_ref() {
            // Uncheck all models
            items.small.set_checked(false);
            items.medium.set_checked(false);
            items.large_v3.set_checked(false);
            items.large_v3_turbo.set_checked(false);

            // Check the selected model
            match variant {
                "small" => items.small.set_checked(true),
                "medium" => items.medium.set_checked(true),
                "large-v3" => items.large_v3.set_checked(true),
                "large-v3-turbo" => items.large_v3_turbo.set_checked(true),
                _ => debug!("Unknown model variant: {}", variant),
            }

            // Update the label text
            let label_text = match variant {
                "small" => "Whisper: Small",
                "medium" => "Whisper: Medium",
                "large-v3" => "Whisper: Large v3",
                "large-v3-turbo" => "Whisper: Large v3 Turbo",
                _ => variant,
            };
            items.label.set_text(label_text);

            info!("Model selection updated to: {}", variant);
        }
    });
}

/// Load the custom CodeScribe icon, optionally tinted by status
fn load_custom_icon(status: TrayStatus) -> Result<Icon> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|e| anyhow::anyhow!("Failed to load icon: {}", e))?;

    // Resize to menu bar size (44x44 for Retina)
    let resized = img.resize_exact(ICON_SIZE, ICON_SIZE, FilterType::Lanczos3);
    let (width, height) = resized.dimensions();
    let mut rgba = resized.to_rgba8().into_raw();

    // Icon stays white/neutral - no tinting
    // Status is shown only via the glyph color

    // Draw status glyph if enabled (larger colored dot in bottom-right corner)
    if is_status_glyph_enabled() {
        // Glyph parameters - larger circle (12x12) for better visibility
        const GLYPH_RADIUS: i32 = 6;
        let glyph_center_x = (width as i32) - GLYPH_RADIUS - 2; // 2px padding from edge
        let glyph_center_y = (height as i32) - GLYPH_RADIUS - 2;

        // Status-based glyph colors:
        // - Green: Idle/Ready, Success
        // - Red: Recording/Listening, Error (X shape)
        // - Orange: Processing/Thinking
        let (glyph_r, glyph_g, glyph_b) = match status {
            TrayStatus::Idle => (80u8, 200, 100),   // Green - ready
            TrayStatus::Listening => (255, 70, 70), // Red - recording
            TrayStatus::Thinking => (255, 165, 0),  // Orange - processing
            TrayStatus::Success => (80, 220, 100),  // Bright green - done
            TrayStatus::Error => (255, 50, 50),     // Bright red - error
        };

        // For Error status, draw an "X" instead of a circle
        if status == TrayStatus::Error {
            // Draw X shape
            const LINE_WIDTH: i32 = 2;
            for y in (glyph_center_y - GLYPH_RADIUS).max(0)
                ..(glyph_center_y + GLYPH_RADIUS).min(height as i32)
            {
                for x in (glyph_center_x - GLYPH_RADIUS).max(0)
                    ..(glyph_center_x + GLYPH_RADIUS).min(width as i32)
                {
                    let dx = x - glyph_center_x;
                    let dy = y - glyph_center_y;

                    // Check if point is on diagonal lines (forming X)
                    let on_diag1 = (dx - dy).abs() <= LINE_WIDTH;
                    let on_diag2 = (dx + dy).abs() <= LINE_WIDTH;

                    // Only draw within the circle bounds
                    let in_bounds = dx * dx + dy * dy <= GLYPH_RADIUS * GLYPH_RADIUS;

                    if in_bounds && (on_diag1 || on_diag2) {
                        let idx = ((y as u32 * width + x as u32) * 4) as usize;
                        rgba[idx] = glyph_r;
                        rgba[idx + 1] = glyph_g;
                        rgba[idx + 2] = glyph_b;
                        rgba[idx + 3] = 255;
                    }
                }
            }
        } else {
            // Draw circle using distance formula
            for y in (glyph_center_y - GLYPH_RADIUS).max(0)
                ..(glyph_center_y + GLYPH_RADIUS).min(height as i32)
            {
                for x in (glyph_center_x - GLYPH_RADIUS).max(0)
                    ..(glyph_center_x + GLYPH_RADIUS).min(width as i32)
                {
                    let dx = x - glyph_center_x;
                    let dy = y - glyph_center_y;
                    let distance_squared = dx * dx + dy * dy;

                    if distance_squared <= GLYPH_RADIUS * GLYPH_RADIUS {
                        let idx = ((y as u32 * width + x as u32) * 4) as usize;
                        rgba[idx] = glyph_r;
                        rgba[idx + 1] = glyph_g;
                        rgba[idx + 2] = glyph_b;
                        rgba[idx + 3] = 255; // Fully opaque
                    }
                }
            }
        }
    }

    Icon::from_rgba(rgba, width, height)
        .map_err(|e| anyhow::anyhow!("Failed to create icon: {}", e))
}

/// Create a simple colored circle icon as fallback
fn create_fallback_icon(status: TrayStatus) -> Result<Icon> {
    const SIZE: u32 = 22;
    const RADIUS: i32 = 10;
    const CENTER: i32 = 11;

    let (r, g, b) = match status {
        TrayStatus::Idle => (100u8, 100, 100),  // Gray
        TrayStatus::Listening => (220, 60, 60), // Red
        TrayStatus::Thinking => (60, 130, 220), // Blue
        TrayStatus::Success => (60, 200, 100),  // Green
        TrayStatus::Error => (255, 50, 50),     // Bright red
    };

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let dx = x - CENTER;
            let dy = y - CENTER;
            if dx * dx + dy * dy <= RADIUS * RADIUS {
                let idx = ((y as u32 * SIZE + x as u32) * 4) as usize;
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE)
        .map_err(|e| anyhow::anyhow!("Failed to create fallback icon: {}", e))
}

/// Status of the CodeScribe system, reflected in tray icon
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    /// Idle, waiting for activation
    Idle,
    /// Actively listening/recording
    Listening,
    /// Processing/transcribing
    Thinking,
    /// Successfully completed
    Success,
    /// Error state - backend not available
    Error,
}

impl TrayStatus {
    /// Get the human-readable tooltip for this status
    pub fn tooltip(&self) -> String {
        match self {
            TrayStatus::Idle => "CodeScribe - Ready".to_string(),
            TrayStatus::Listening => "CodeScribe - Recording...".to_string(),
            TrayStatus::Thinking => "CodeScribe - Processing...".to_string(),
            TrayStatus::Success => "CodeScribe - Done!".to_string(),
            TrayStatus::Error => "CodeScribe - Backend unavailable!".to_string(),
        }
    }

    /// Create an icon from this status using the custom CodeScribe logo
    /// Falls back to simple circle if custom icon fails
    fn to_icon(self) -> Result<Icon> {
        load_custom_icon(self).or_else(|e| {
            debug!("Custom icon failed, using fallback: {}", e);
            create_fallback_icon(self)
        })
    }
}

/// Menu events that can be sent to the main controller.
///
/// ## Event Handling Status
///
/// These events are sent via `send_menu_event()` and should be received by
/// calling `menu_event_receiver()` in the main controller. See `main.rs` for
/// the event handling loop.
///
/// **TODO(unwired)**: Several events need full implementation:
/// - `ToggleHotkeys` - needs to enable/disable global hotkey listener
/// - `SetLanguage` - needs to persist to config and update backend
/// - `SetWhisperModel` - needs model download/switch logic
/// - `SetFormattingProvider` - needs to persist to config
/// - `ToggleAiFormatting` - needs to persist to config
/// - `SetHoldMods` - needs to persist and update hotkey listener
/// - `ToggleHoldExclusive` - needs to persist to config
/// - `SetToggleTrigger` - needs to persist and update hotkey listener
/// - `ToggleHistory` - needs to persist to config
/// - `CopyLatestToClipboard` - needs history integration
/// - `SelectHistoryEntry` - needs history submenu population
/// - `ToggleStatusGlyph` - needs icon rendering update
/// - `RefreshTrayIcon` - implemented inline
/// - `ToggleStartSound` - needs to persist to config
/// - `SetSoundType` - needs to persist to config
/// - `SetVolume` - needs volume UI dialog
#[derive(Debug, Clone)]
pub enum TrayMenuEvent {
    // Top-level actions
    ToggleHotkeys,
    StartAtLogin(bool),
    Quit,

    // Language submenu
    SetLanguage(Language),

    // Models submenu (Whisper model selection)
    SetWhisperModel(WhisperModel),
    OpenModelsFolder,

    // Formatting submenu
    SetFormattingProvider(FormattingProvider),
    ToggleAiFormatting,

    // Hold Hotkeys submenu
    SetHoldMods(HoldMods),
    ToggleHoldExclusive,
    SetToggleTrigger(ToggleTrigger),

    // History submenu
    ToggleHistory,
    CopyLatestToClipboard,
    OpenHistoryFolder,
    SelectHistoryEntry(usize),

    // Appearance submenu
    ToggleStatusGlyph,
    RefreshTrayIcon,

    // Feedback submenu
    ToggleStartSound,
    SetSoundType(SoundType),
    SetVolume(VolumeLevel),

    // Permissions submenu
    CheckPermissions,
    OpenAccessibilitySettings,
    OpenMicrophoneSettings,

    // Tools submenu
    OpenVoiceLab,
    OpenTeacher,
}

// FormattingProvider is tray-specific (maps to config::AiProvider)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattingProvider {
    Harmony,
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundType {
    Tink,
    Pop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeLevel {
    Mute,   // 0%
    Low,    // 25%
    Medium, // 50%
    High,   // 75%
    Full,   // 100%
}

impl VolumeLevel {
    /// Convert to f32 value (0.0 - 1.0)
    pub fn as_f32(self) -> f32 {
        match self {
            VolumeLevel::Mute => 0.0,
            VolumeLevel::Low => 0.25,
            VolumeLevel::Medium => 0.5,
            VolumeLevel::High => 0.75,
            VolumeLevel::Full => 1.0,
        }
    }

    /// Get display label
    pub fn label(self) -> &'static str {
        match self {
            VolumeLevel::Mute => "🔇 Mute (0%)",
            VolumeLevel::Low => "🔈 Low (25%)",
            VolumeLevel::Medium => "🔉 Medium (50%)",
            VolumeLevel::High => "🔊 High (75%)",
            VolumeLevel::Full => "🔊 Full (100%)",
        }
    }

    /// Get VolumeLevel from f32 value (rounds to nearest)
    pub fn from_f32(value: f32) -> Self {
        if value <= 0.125 {
            VolumeLevel::Mute
        } else if value <= 0.375 {
            VolumeLevel::Low
        } else if value <= 0.625 {
            VolumeLevel::Medium
        } else if value <= 0.875 {
            VolumeLevel::High
        } else {
            VolumeLevel::Full
        }
    }
}

/// Whisper model variants available for local STT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperModel {
    Small,
    Medium,
    LargeV3,
    LargeV3Turbo,
}

impl WhisperModel {
    /// Human-readable label for the menu
    pub fn label(&self) -> &'static str {
        match self {
            WhisperModel::Small => "Small",
            WhisperModel::Medium => "Medium",
            WhisperModel::LargeV3 => "Large v3",
            WhisperModel::LargeV3Turbo => "Large v3 Turbo",
        }
    }

    /// Directory name / model identifier
    pub fn model_id(&self) -> &'static str {
        match self {
            WhisperModel::Small => "whisper-small",
            WhisperModel::Medium => "whisper-medium",
            WhisperModel::LargeV3 => "whisper-large-v3",
            WhisperModel::LargeV3Turbo => "whisper-large-v3-turbo",
        }
    }
}

/// Menu item IDs for tracking all clickable items
struct MenuIds {
    // Top-level
    enable_hotkeys: MenuId,
    start_at_login: MenuId,
    quit: MenuId,

    // Language submenu
    lang_auto: MenuId,
    lang_polish: MenuId,
    lang_english: MenuId,

    // Models submenu (Whisper model selection)
    model_small: MenuId,
    model_medium: MenuId,
    model_large_v3: MenuId,
    model_large_v3_turbo: MenuId,
    model_open_folder: MenuId,

    // Formatting submenu
    fmt_toggle: MenuId,
    fmt_harmony: MenuId,
    fmt_ollama: MenuId,

    // Hold Hotkeys submenu
    hold_ctrl: MenuId,
    hold_ctrl_opt: MenuId,
    hold_ctrl_shift: MenuId,
    hold_ctrl_cmd: MenuId,
    hold_exclusive: MenuId,
    toggle_double_opt: MenuId,
    toggle_double_ralt: MenuId,
    toggle_disabled: MenuId,

    // History submenu
    history_save: MenuId,
    history_copy_latest: MenuId,
    history_open_folder: MenuId,

    // Appearance submenu
    appearance_glyph: MenuId,
    appearance_refresh: MenuId,

    // Feedback submenu
    feedback_start_sound: MenuId,
    feedback_sound_tink: MenuId,
    feedback_sound_pop: MenuId,
    volume_mute: MenuId,
    volume_low: MenuId,
    volume_medium: MenuId,
    volume_high: MenuId,
    volume_full: MenuId,

    // Permissions submenu
    perm_check: MenuId,
    perm_accessibility: MenuId,
    perm_microphone: MenuId,

    // Tools submenu
    tools_voice_lab: MenuId,
    tools_teacher: MenuId,
}

/// Build the complete tray menu with all submenus
fn build_menu() -> Result<(Menu, MenuIds)> {
    let menu = Menu::new();

    // 1. Status: Ready (disabled label)
    let status_item = MenuItem::new("Status: Ready", false, None);
    menu.append(&status_item)?;

    // 2. Enable Hotkeys (checkbox toggle)
    let enable_hotkeys = CheckMenuItem::new("Enable Hotkeys", true, true, None);
    let enable_hotkeys_id = enable_hotkeys.id().clone();
    menu.append(&enable_hotkeys)?;

    // 3. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 4. Language submenu
    let lang_menu = Submenu::new("Language", true);
    let lang_auto = CheckMenuItem::new("Auto", true, true, None);
    let lang_auto_id = lang_auto.id().clone();
    let lang_polish = CheckMenuItem::new("Polish (PL)", true, false, None);
    let lang_polish_id = lang_polish.id().clone();
    let lang_english = CheckMenuItem::new("English (EN)", true, false, None);
    let lang_english_id = lang_english.id().clone();

    lang_menu.append(&lang_auto)?;
    lang_menu.append(&lang_polish)?;
    lang_menu.append(&lang_english)?;
    menu.append(&lang_menu)?;

    // 5. Models submenu (Whisper model selection)
    let models_menu = Submenu::new("Models", true);

    // Current Whisper model label (read from env or default)
    let current_whisper = std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "small".to_string());
    let current_label = match current_whisper.as_str() {
        "small" => "Small",
        "medium" => "Medium",
        "large-v3" => "Large v3",
        "large-v3-turbo" => "Large v3 Turbo",
        _ => &current_whisper,
    };
    let whisper_label = MenuItem::new(format!("Whisper: {}", current_label), false, None);
    models_menu.append(&whisper_label)?;
    models_menu.append(&PredefinedMenuItem::separator())?;

    // Whisper model options (using CheckMenuItem for dynamic updates)
    let model_small =
        CheckMenuItem::new("Use Whisper: Small", true, current_whisper == "small", None);
    let model_small_id = model_small.id().clone();
    let model_medium = CheckMenuItem::new(
        "Use Whisper: Medium",
        true,
        current_whisper == "medium",
        None,
    );
    let model_medium_id = model_medium.id().clone();
    let model_large_v3 = CheckMenuItem::new(
        "Use Whisper: Large v3",
        true,
        current_whisper == "large-v3",
        None,
    );
    let model_large_v3_id = model_large_v3.id().clone();
    let model_large_v3_turbo = CheckMenuItem::new(
        "Use Whisper: Large v3 Turbo",
        true,
        current_whisper == "large-v3-turbo",
        None,
    );
    let model_large_v3_turbo_id = model_large_v3_turbo.id().clone();

    models_menu.append(&model_small)?;
    models_menu.append(&model_medium)?;
    models_menu.append(&model_large_v3)?;
    models_menu.append(&model_large_v3_turbo)?;
    models_menu.append(&PredefinedMenuItem::separator())?;

    // Open Models Folder
    let model_open_folder = MenuItem::new("Open Models Folder", true, None);
    let model_open_folder_id = model_open_folder.id().clone();
    models_menu.append(&model_open_folder)?;

    menu.append(&models_menu)?;

    // Store model menu items for dynamic updates (main thread only)
    MODEL_MENU_ITEMS.with(|items_cell| {
        *items_cell.borrow_mut() = Some(ModelMenuItems {
            small: model_small,
            medium: model_medium,
            large_v3: model_large_v3,
            large_v3_turbo: model_large_v3_turbo,
            label: whisper_label,
        });
    });

    // 6. Formatting submenu (AI enhancement)
    let fmt_menu = Submenu::new("Formatting", true);

    // AI formatting toggle
    let ai_enabled = std::env::var("FORMAT_ENABLED")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);
    let fmt_toggle = CheckMenuItem::new("Enable AI Formatting", true, ai_enabled, None);
    let fmt_toggle_id = fmt_toggle.id().clone();
    fmt_menu.append(&fmt_toggle)?;
    fmt_menu.append(&PredefinedMenuItem::separator())?;

    // Provider selection
    let fmt_provider_label = MenuItem::new("Provider", false, None);
    fmt_menu.append(&fmt_provider_label)?;

    let current_provider = std::env::var("AI_PROVIDER").unwrap_or_else(|_| "harmony".to_string());
    let fmt_harmony = CheckMenuItem::new(
        "Harmony (LibraxisAI)",
        true,
        current_provider == "harmony",
        None,
    );
    let fmt_harmony_id = fmt_harmony.id().clone();
    let fmt_ollama = CheckMenuItem::new("Ollama (Local)", true, current_provider == "ollama", None);
    let fmt_ollama_id = fmt_ollama.id().clone();

    fmt_menu.append(&fmt_harmony)?;
    fmt_menu.append(&fmt_ollama)?;
    fmt_menu.append(&PredefinedMenuItem::separator())?;

    // Assistive mode info
    let assistive_label = MenuItem::new("Assistive: Ctrl+Shift → AI chat mode", false, None);
    fmt_menu.append(&assistive_label)?;

    menu.append(&fmt_menu)?;

    // 7. Hold Hotkeys submenu
    let hold_menu = Submenu::new("Hold Hotkeys", true);

    // Current: [label]
    let hold_current_label = MenuItem::new("Current: Ctrl only (Formatting)", false, None);
    hold_menu.append(&hold_current_label)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    // Hold modifier options (uses label() from config types)
    let hold_ctrl = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::Ctrl.label()),
        true,
        true,
        None,
    );
    let hold_ctrl_id = hold_ctrl.id().clone();
    let hold_ctrl_opt = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlAlt.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_opt_id = hold_ctrl_opt.id().clone();
    let hold_ctrl_shift = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlShift.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_shift_id = hold_ctrl_shift.id().clone();
    let hold_ctrl_cmd = CheckMenuItem::new(
        format!("Hold: {}", HoldMods::CtrlCmd.label()),
        true,
        false,
        None,
    );
    let hold_ctrl_cmd_id = hold_ctrl_cmd.id().clone();

    hold_menu.append(&hold_ctrl)?;
    hold_menu.append(&hold_ctrl_opt)?;
    hold_menu.append(&hold_ctrl_shift)?;
    hold_menu.append(&hold_ctrl_cmd)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    // Exclusive checkbox
    let hold_exclusive = CheckMenuItem::new("Exclusive (ignore extra modifiers)", true, true, None);
    let hold_exclusive_id = hold_exclusive.id().clone();
    hold_menu.append(&hold_exclusive)?;
    hold_menu.append(&PredefinedMenuItem::separator())?;

    // Toggle trigger options
    let toggle_label = MenuItem::new("Toggle: double option", false, None);
    hold_menu.append(&toggle_label)?;
    let toggle_double_opt = CheckMenuItem::new("Use double Option (⌥⌥)", true, true, None);
    let toggle_double_opt_id = toggle_double_opt.id().clone();
    let toggle_double_ralt = CheckMenuItem::new("Use double Right Option", true, false, None);
    let toggle_double_ralt_id = toggle_double_ralt.id().clone();
    let toggle_disabled = CheckMenuItem::new("Disable toggle", true, false, None);
    let toggle_disabled_id = toggle_disabled.id().clone();

    hold_menu.append(&toggle_double_opt)?;
    hold_menu.append(&toggle_double_ralt)?;
    hold_menu.append(&toggle_disabled)?;

    menu.append(&hold_menu)?;

    // 7. History submenu
    let history_menu = Submenu::new("History", true);

    // Load actual history entries at startup
    let recent_entries = crate::history::recent_entries(5);
    let latest_label = if let Some(entry) = recent_entries.first() {
        format!("Latest: {}", entry.label())
    } else {
        "Latest: (none)".to_string()
    };
    let history_latest_label = MenuItem::new(latest_label, false, None);
    history_menu.append(&history_latest_label)?;
    history_menu.append(&PredefinedMenuItem::separator())?;

    let history_save = CheckMenuItem::new("Save transcripts to History", true, true, None);
    let history_save_id = history_save.id().clone();
    history_menu.append(&history_save)?;
    history_menu.append(&PredefinedMenuItem::separator())?;

    // Show recent entries or placeholder
    if recent_entries.is_empty() {
        let placeholder_entry = MenuItem::new("(no recent entries)", false, None);
        history_menu.append(&placeholder_entry)?;
    } else {
        for (i, entry) in recent_entries.iter().take(5).enumerate() {
            // Truncate label for menu display (UTF-8 safe)
            let label = entry.label();
            let display = if label.chars().count() > 40 {
                let truncated: String = label.chars().take(37).collect();
                format!("{}...", truncated)
            } else {
                label.to_string()
            };
            let entry_item = MenuItem::new(display, true, None);
            // Note: These won't have handlers until we add dynamic menu IDs
            // For now they're display-only
            history_menu.append(&entry_item)?;
            let _ = i; // suppress unused warning
        }
    }
    history_menu.append(&PredefinedMenuItem::separator())?;

    let history_copy_latest = MenuItem::new("Copy Latest to Clipboard", true, None);
    let history_copy_latest_id = history_copy_latest.id().clone();
    let history_open_folder = MenuItem::new("Open History Folder", true, None);
    let history_open_folder_id = history_open_folder.id().clone();

    history_menu.append(&history_copy_latest)?;
    history_menu.append(&history_open_folder)?;

    menu.append(&history_menu)?;

    // 8. Appearance submenu
    let appearance_menu = Submenu::new("Appearance", true);

    let appearance_glyph = CheckMenuItem::new("Show status glyph next to icon", true, true, None);
    let appearance_glyph_id = appearance_glyph.id().clone();
    appearance_menu.append(&appearance_glyph)?;
    appearance_menu.append(&PredefinedMenuItem::separator())?;

    let appearance_refresh = MenuItem::new("Refresh Tray Icon", true, None);
    let appearance_refresh_id = appearance_refresh.id().clone();
    appearance_menu.append(&appearance_refresh)?;

    menu.append(&appearance_menu)?;

    // 9. Feedback submenu
    let feedback_menu = Submenu::new("Feedback", true);

    let feedback_start_sound = CheckMenuItem::new("Enable Start Sound", true, true, None);
    let feedback_start_sound_id = feedback_start_sound.id().clone();
    feedback_menu.append(&feedback_start_sound)?;
    feedback_menu.append(&PredefinedMenuItem::separator())?;

    let feedback_sound_tink = CheckMenuItem::new("Sound: Tink", true, true, None);
    let feedback_sound_tink_id = feedback_sound_tink.id().clone();
    let feedback_sound_pop = CheckMenuItem::new("Sound: Pop", true, false, None);
    let feedback_sound_pop_id = feedback_sound_pop.id().clone();
    feedback_menu.append(&feedback_sound_tink)?;
    feedback_menu.append(&feedback_sound_pop)?;

    // Volume submenu with preset levels
    let volume_menu = Submenu::new("Volume", true);
    let volume_mute = CheckMenuItem::new(VolumeLevel::Mute.label(), true, false, None);
    let volume_mute_id = volume_mute.id().clone();
    let volume_low = CheckMenuItem::new(VolumeLevel::Low.label(), true, false, None);
    let volume_low_id = volume_low.id().clone();
    let volume_medium = CheckMenuItem::new(VolumeLevel::Medium.label(), true, true, None); // Default
    let volume_medium_id = volume_medium.id().clone();
    let volume_high = CheckMenuItem::new(VolumeLevel::High.label(), true, false, None);
    let volume_high_id = volume_high.id().clone();
    let volume_full = CheckMenuItem::new(VolumeLevel::Full.label(), true, false, None);
    let volume_full_id = volume_full.id().clone();
    volume_menu.append(&volume_mute)?;
    volume_menu.append(&volume_low)?;
    volume_menu.append(&volume_medium)?;
    volume_menu.append(&volume_high)?;
    volume_menu.append(&volume_full)?;
    feedback_menu.append(&volume_menu)?;

    menu.append(&feedback_menu)?;

    // 10. Tools submenu (Voice Lab, Teacher)
    let tools_menu = Submenu::new("Tools", true);

    let tools_voice_lab = MenuItem::new("🔬 Open Voice Lab", true, None);
    let tools_voice_lab_id = tools_voice_lab.id().clone();
    tools_menu.append(&tools_voice_lab)?;

    let tools_teacher = MenuItem::new("👨‍🏫 Calibration Teacher", true, None);
    let tools_teacher_id = tools_teacher.id().clone();
    tools_menu.append(&tools_teacher)?;

    menu.append(&tools_menu)?;

    // 12. Permissions submenu
    let permissions_menu = Submenu::new("Permissions", true);

    // Status display using permission check functions
    let ax_status = if crate::permissions::check_accessibility()
        == crate::permissions::PermissionStatus::Granted
    {
        "✓"
    } else {
        "✗"
    };
    let mic_status = match crate::permissions::check_microphone() {
        crate::permissions::PermissionStatus::Granted => "✓",
        crate::permissions::PermissionStatus::NotDetermined => "?",
        _ => "✗",
    };
    let perm_status_label = MenuItem::new(
        format!("AX: {} | Mic: {}", ax_status, mic_status),
        false,
        None,
    );
    permissions_menu.append(&perm_status_label)?;
    permissions_menu.append(&PredefinedMenuItem::separator())?;

    let perm_check = MenuItem::new("Check Permissions Now", true, None);
    let perm_check_id = perm_check.id().clone();
    permissions_menu.append(&perm_check)?;
    permissions_menu.append(&PredefinedMenuItem::separator())?;

    let perm_accessibility = MenuItem::new("Open Accessibility Settings", true, None);
    let perm_accessibility_id = perm_accessibility.id().clone();
    permissions_menu.append(&perm_accessibility)?;

    let perm_microphone = MenuItem::new("Open Microphone Settings", true, None);
    let perm_microphone_id = perm_microphone.id().clone();
    permissions_menu.append(&perm_microphone)?;

    menu.append(&permissions_menu)?;

    // 13. Separator
    menu.append(&PredefinedMenuItem::separator())?;

    // 14. Start at Login (checkbox) - check current state from launchd
    let is_enabled = crate::launchd::is_login_item_enabled();
    let start_at_login = CheckMenuItem::new("Start at Login", true, is_enabled, None);
    let start_at_login_id = start_at_login.id().clone();
    menu.append(&start_at_login)?;

    // 15. Quit
    let quit_item = MenuItem::new("Quit", true, None);
    let quit_id = quit_item.id().clone();
    menu.append(&quit_item)?;

    Ok((
        menu,
        MenuIds {
            enable_hotkeys: enable_hotkeys_id,
            start_at_login: start_at_login_id,
            quit: quit_id,
            lang_auto: lang_auto_id,
            lang_polish: lang_polish_id,
            lang_english: lang_english_id,
            model_small: model_small_id,
            model_medium: model_medium_id,
            model_large_v3: model_large_v3_id,
            model_large_v3_turbo: model_large_v3_turbo_id,
            model_open_folder: model_open_folder_id,
            fmt_toggle: fmt_toggle_id,
            fmt_harmony: fmt_harmony_id,
            fmt_ollama: fmt_ollama_id,
            hold_ctrl: hold_ctrl_id,
            hold_ctrl_opt: hold_ctrl_opt_id,
            hold_ctrl_shift: hold_ctrl_shift_id,
            hold_ctrl_cmd: hold_ctrl_cmd_id,
            hold_exclusive: hold_exclusive_id,
            toggle_double_opt: toggle_double_opt_id,
            toggle_double_ralt: toggle_double_ralt_id,
            toggle_disabled: toggle_disabled_id,
            history_save: history_save_id,
            history_copy_latest: history_copy_latest_id,
            history_open_folder: history_open_folder_id,
            appearance_glyph: appearance_glyph_id,
            appearance_refresh: appearance_refresh_id,
            feedback_start_sound: feedback_start_sound_id,
            feedback_sound_tink: feedback_sound_tink_id,
            feedback_sound_pop: feedback_sound_pop_id,
            volume_mute: volume_mute_id,
            volume_low: volume_low_id,
            volume_medium: volume_medium_id,
            volume_high: volume_high_id,
            volume_full: volume_full_id,
            perm_check: perm_check_id,
            perm_accessibility: perm_accessibility_id,
            perm_microphone: perm_microphone_id,
            tools_voice_lab: tools_voice_lab_id,
            tools_teacher: tools_teacher_id,
        },
    ))
}

/// Global channel for status updates (crossbeam for sync safety)
static STATUS_CHANNEL: OnceLock<Sender<TrayStatus>> = OnceLock::new();

/// Global channel for menu events
static MENU_EVENT_CHANNEL: OnceLock<Sender<TrayMenuEvent>> = OnceLock::new();

/// Update the tray icon to reflect current status
pub fn update_tray_status(status: TrayStatus) -> Result<()> {
    if let Some(sender) = STATUS_CHANNEL.get() {
        sender
            .send(status)
            .map_err(|e| anyhow::anyhow!("Failed to send tray status: {}", e))?;
        debug!("Tray status update sent: {:?}", status);
        Ok(())
    } else {
        debug!("Tray status channel not initialized yet");
        Ok(())
    }
}

/// Get a receiver for menu events (call once from main controller)
pub fn menu_event_receiver() -> Result<Receiver<TrayMenuEvent>> {
    let (tx, rx) = unbounded();
    MENU_EVENT_CHANNEL
        .set(tx)
        .map_err(|_| anyhow::anyhow!("Menu event channel already initialized"))?;
    Ok(rx)
}

/// Send a menu event to the main controller
fn send_menu_event(event: TrayMenuEvent) {
    if let Some(sender) = MENU_EVENT_CHANNEL.get() {
        if let Err(e) = sender.send(event) {
            debug!("Failed to send menu event: {}", e);
        }
    }
}

/// Handle menu item click and send appropriate event
fn handle_menu_event(event_id: &MenuId, menu_ids: &MenuIds) {
    // Top-level actions
    if event_id == &menu_ids.enable_hotkeys {
        send_menu_event(TrayMenuEvent::ToggleHotkeys);
    } else if event_id == &menu_ids.start_at_login {
        // Toggle based on current state
        let current = crate::launchd::is_login_item_enabled();
        send_menu_event(TrayMenuEvent::StartAtLogin(!current));
    } else if event_id == &menu_ids.quit {
        send_menu_event(TrayMenuEvent::Quit);
    }
    // Language submenu
    else if event_id == &menu_ids.lang_auto {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::Auto));
    } else if event_id == &menu_ids.lang_polish {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::Polish));
    } else if event_id == &menu_ids.lang_english {
        send_menu_event(TrayMenuEvent::SetLanguage(Language::English));
    }
    // Models submenu (Whisper model selection)
    else if event_id == &menu_ids.model_small {
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::Small));
    } else if event_id == &menu_ids.model_medium {
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::Medium));
    } else if event_id == &menu_ids.model_large_v3 {
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::LargeV3));
    } else if event_id == &menu_ids.model_large_v3_turbo {
        send_menu_event(TrayMenuEvent::SetWhisperModel(WhisperModel::LargeV3Turbo));
    } else if event_id == &menu_ids.model_open_folder {
        send_menu_event(TrayMenuEvent::OpenModelsFolder);
        // Open models folder in Finder
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            // Try to open the models directory
            if let Ok(home) = std::env::var("HOME") {
                let models_path = format!("{}/.CodeScribe/models", home);
                // Create if not exists
                let _ = std::fs::create_dir_all(&models_path);
                let _ = Command::new("open").arg(&models_path).spawn();
            }
        }
    }
    // Formatting submenu
    else if event_id == &menu_ids.fmt_toggle {
        send_menu_event(TrayMenuEvent::ToggleAiFormatting);
    } else if event_id == &menu_ids.fmt_harmony {
        send_menu_event(TrayMenuEvent::SetFormattingProvider(
            FormattingProvider::Harmony,
        ));
    } else if event_id == &menu_ids.fmt_ollama {
        send_menu_event(TrayMenuEvent::SetFormattingProvider(
            FormattingProvider::Ollama,
        ));
    }
    // Hold Hotkeys submenu
    else if event_id == &menu_ids.hold_ctrl {
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::Ctrl));
    } else if event_id == &menu_ids.hold_ctrl_opt {
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlAlt));
    } else if event_id == &menu_ids.hold_ctrl_shift {
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlShift));
    } else if event_id == &menu_ids.hold_ctrl_cmd {
        send_menu_event(TrayMenuEvent::SetHoldMods(HoldMods::CtrlCmd));
    } else if event_id == &menu_ids.hold_exclusive {
        send_menu_event(TrayMenuEvent::ToggleHoldExclusive);
    } else if event_id == &menu_ids.toggle_double_opt {
        send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::DoubleOption));
    } else if event_id == &menu_ids.toggle_double_ralt {
        send_menu_event(TrayMenuEvent::SetToggleTrigger(
            ToggleTrigger::DoubleRightOption,
        ));
    } else if event_id == &menu_ids.toggle_disabled {
        send_menu_event(TrayMenuEvent::SetToggleTrigger(ToggleTrigger::None));
    }
    // History submenu
    else if event_id == &menu_ids.history_save {
        send_menu_event(TrayMenuEvent::ToggleHistory);
    } else if event_id == &menu_ids.history_copy_latest {
        send_menu_event(TrayMenuEvent::CopyLatestToClipboard);
    } else if event_id == &menu_ids.history_open_folder {
        send_menu_event(TrayMenuEvent::OpenHistoryFolder);
    }
    // Appearance submenu
    else if event_id == &menu_ids.appearance_glyph {
        send_menu_event(TrayMenuEvent::ToggleStatusGlyph);
    } else if event_id == &menu_ids.appearance_refresh {
        send_menu_event(TrayMenuEvent::RefreshTrayIcon);
    }
    // Feedback submenu
    else if event_id == &menu_ids.feedback_start_sound {
        send_menu_event(TrayMenuEvent::ToggleStartSound);
    } else if event_id == &menu_ids.feedback_sound_tink {
        send_menu_event(TrayMenuEvent::SetSoundType(SoundType::Tink));
    } else if event_id == &menu_ids.feedback_sound_pop {
        send_menu_event(TrayMenuEvent::SetSoundType(SoundType::Pop));
    }
    // Volume submenu
    else if event_id == &menu_ids.volume_mute {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Mute));
    } else if event_id == &menu_ids.volume_low {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Low));
    } else if event_id == &menu_ids.volume_medium {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Medium));
    } else if event_id == &menu_ids.volume_high {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::High));
    } else if event_id == &menu_ids.volume_full {
        send_menu_event(TrayMenuEvent::SetVolume(VolumeLevel::Full));
    }
    // Permissions submenu
    else if event_id == &menu_ids.perm_check {
        send_menu_event(TrayMenuEvent::CheckPermissions);
        // Also log current status immediately
        crate::permissions::check_all_permissions();
    } else if event_id == &menu_ids.perm_accessibility {
        send_menu_event(TrayMenuEvent::OpenAccessibilitySettings);
        // Open System Settings > Privacy & Security > Accessibility
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let _ = Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .spawn();
        }
    } else if event_id == &menu_ids.perm_microphone {
        send_menu_event(TrayMenuEvent::OpenMicrophoneSettings);
        // Open System Settings > Privacy & Security > Microphone
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let _ = Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
                .spawn();
        }
    }
    // Tools submenu
    else if event_id == &menu_ids.tools_voice_lab {
        send_menu_event(TrayMenuEvent::OpenVoiceLab);
        // Open Voice Lab in browser (backend /tester endpoint)
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let backend_url = std::env::var("CODESCRIBE_BACKEND_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8237".to_string());
            let lab_url = format!("{}/tester", backend_url);
            info!("Opening Voice Lab: {}", lab_url);
            let _ = Command::new("open").arg(&lab_url).spawn();
        }
    } else if event_id == &menu_ids.tools_teacher {
        send_menu_event(TrayMenuEvent::OpenTeacher);
        // Open Teacher/Calibration in browser
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let backend_url = std::env::var("CODESCRIBE_BACKEND_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8237".to_string());
            // Teacher mode uses the same Lab UI but with calibration wizard
            let teacher_url = format!("{}/tester#calibrate", backend_url);
            info!("Opening Calibration Teacher: {}", teacher_url);
            let _ = Command::new("open").arg(&teacher_url).spawn();
        }
    } else {
        // Unknown menu event - log for debugging
        debug!("Unknown menu event id: {:?}", event_id);
    }
}

/// Global shutdown flag for graceful exit
static SHUTDOWN_REQUESTED: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();

/// Request graceful shutdown of the tray application.
///
/// This can be called from any thread to signal that the app should exit.
/// The event loop will check this flag and perform cleanup before exiting.
pub fn request_shutdown() {
    if let Some(flag) = SHUTDOWN_REQUESTED.get() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        info!("Shutdown requested");
    }
}

/// Check if shutdown has been requested
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED
        .get()
        .map(|f| f.load(std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(false)
}

/// Run the tray application (blocking)
///
/// Uses tao event loop for proper macOS integration.
/// Optionally accepts a HotkeyManager to process hotkey events in the same loop.
pub fn run() -> Result<()> {
    run_with_hotkeys(None)
}

/// Run the tray application with optional hotkey manager
///
/// The hotkey manager must be created on main thread before calling this.
///
/// ## Shutdown Behavior
///
/// The event loop will exit when:
/// - User clicks Quit in the tray menu
/// - `request_shutdown()` is called from any thread
/// - Status channel is disconnected
///
/// On exit, cleanup is performed:
/// - Hotkey manager is dropped (unregisters hotkeys)
/// - Tray icon is removed
/// - All channels are closed
pub fn run_with_hotkeys(hotkey_manager: Option<crate::hotkeys::HotkeyManager>) -> Result<()> {
    info!("Initializing system tray...");

    // Initialize shutdown flag
    SHUTDOWN_REQUESTED.get_or_init(|| std::sync::atomic::AtomicBool::new(false));

    // Create channel for status updates (crossbeam for sync safety)
    let (status_tx, status_rx): (Sender<TrayStatus>, Receiver<TrayStatus>) = unbounded();
    STATUS_CHANNEL
        .set(status_tx)
        .map_err(|_| anyhow::anyhow!("Status channel already initialized"))?;

    // Create channel for model selection updates (from async tasks to main thread)
    let (model_tx, model_rx): (Sender<String>, Receiver<String>) = unbounded();
    MODEL_UPDATE_CHANNEL
        .set(model_tx)
        .map_err(|_| anyhow::anyhow!("Model update channel already initialized"))?;

    // Build event loop (must be on main thread for macOS)
    let event_loop = EventLoopBuilder::new().build();

    // Build the menu and get IDs
    let (menu, menu_ids) = build_menu()?;

    // Create initial icon
    let initial_status = TrayStatus::Idle;
    let icon = initial_status.to_icon()?;

    // Build the tray icon
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(initial_status.tooltip())
        .with_icon(icon)
        .build()?;

    info!("System tray initialized");

    // Get menu event receiver
    let menu_channel = MenuEvent::receiver();

    if hotkey_manager.is_some() {
        info!("Global hotkeys enabled");
    }

    info!("Starting tray event loop...");
    info!("Press Quit in the tray menu to exit");

    // Poll interval for checking channels
    let poll_interval = Duration::from_millis(100);

    // Run the event loop
    event_loop.run(move |_event, _, control_flow| {
        // Use WaitUntil to avoid busy-waiting while still checking channels
        *control_flow = ControlFlow::WaitUntil(Instant::now() + poll_interval);

        // Check for programmatic shutdown request
        if is_shutdown_requested() {
            info!("Shutdown flag detected, performing cleanup...");
            // Cleanup will happen when tray_icon and hotkey_manager are dropped
            *control_flow = ControlFlow::Exit;
            return;
        }

        // Process hotkey events (integrated with main event loop for macOS compatibility)
        if let Some(ref hk_manager) = hotkey_manager {
            hk_manager.process_events();
        }

        // Check for status updates (non-blocking)
        match status_rx.try_recv() {
            Ok(new_status) => {
                debug!("Received status update: {:?}", new_status);

                // Update tooltip
                if let Err(e) = tray_icon.set_tooltip(Some(new_status.tooltip())) {
                    debug!("Failed to update tray tooltip: {}", e);
                }

                // Update icon
                if let Ok(new_icon) = new_status.to_icon() {
                    if let Err(e) = tray_icon.set_icon(Some(new_icon)) {
                        debug!("Failed to update tray icon: {}", e);
                    }
                }

                info!("Tray status updated to: {:?}", new_status);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                info!("Status channel closed, exiting");
                *control_flow = ControlFlow::Exit;
            }
        }

        // Check for model selection updates (from async tasks)
        if let Ok(variant) = model_rx.try_recv() {
            apply_model_selection(&variant);
        }

        // Check for menu events (non-blocking)
        if let Ok(event) = menu_channel.try_recv() {
            debug!("Menu event received: id={:?}", event.id);
            // Handle menu item clicks
            handle_menu_event(&event.id, &menu_ids);

            // Handle Quit specially to exit event loop
            if event.id == menu_ids.quit {
                info!("Quit requested via menu, exiting...");
                *control_flow = ControlFlow::Exit;
            }
        }
    });

    // Note: This code is unreachable because event_loop.run() never returns
    // on macOS. Cleanup happens when the closures are dropped.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_creation() {
        let icon = TrayStatus::Idle.to_icon();
        assert!(icon.is_ok());
    }

    #[test]
    fn test_status_tooltips() {
        assert_eq!(TrayStatus::Idle.tooltip(), "CodeScribe - Ready");
        assert_eq!(TrayStatus::Listening.tooltip(), "CodeScribe - Recording...");
        assert_eq!(TrayStatus::Thinking.tooltip(), "CodeScribe - Processing...");
        assert_eq!(TrayStatus::Success.tooltip(), "CodeScribe - Done!");
    }

    #[test]
    fn test_hold_mods_labels() {
        assert_eq!(HoldMods::Ctrl.label(), "Ctrl only (Formatting)");
        assert_eq!(HoldMods::CtrlAlt.label(), "Ctrl+Option");
        assert_eq!(HoldMods::CtrlShift.label(), "Ctrl+Shift (AI)");
        assert_eq!(HoldMods::CtrlCmd.label(), "Ctrl+Command");
    }

    #[test]
    fn test_toggle_trigger_labels() {
        assert_eq!(ToggleTrigger::DoubleOption.label(), "double option");
        assert_eq!(
            ToggleTrigger::DoubleRightOption.label(),
            "double right option"
        );
        assert_eq!(ToggleTrigger::None.label(), "disabled");
    }
}
