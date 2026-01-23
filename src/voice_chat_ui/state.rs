//! Voice Chat UI state and types
//!
//! Contains overlay state, configuration, and message types.

use std::sync::{Arc, Mutex};

/// Type alias for voice chat send callback
pub type VoiceChatSendCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Configuration for the voice chat overlay
#[derive(Debug, Clone)]
pub struct VoiceChatOverlayConfig {
    /// Width of the overlay window in pixels
    pub width: f64,
    /// Height of the overlay window in pixels
    pub height: f64,
    /// Auto-hide timeout in seconds (0 = no auto-hide)
    pub auto_hide_timeout_secs: u64,
}

impl Default for VoiceChatOverlayConfig {
    fn default() -> Self {
        Self {
            width: 750.0,  // Mission Control: split view (60% left + 40% right)
            height: 400.0, // Increased for better chat history visibility
            auto_hide_timeout_secs: 5,
        }
    }
}

/// Source of the message input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSource {
    Voice,
    Manual,
}

/// Role of a chat message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

/// A single chat message
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    pub is_streaming: bool,
    pub is_error: bool,
}

/// Voice chat overlay state
pub struct VoiceChatOverlayState {
    // UI element handles (stored as usize for FFI safety)
    pub window: Option<usize>,
    pub window_delegate: Option<usize>,
    pub scroll_view: Option<usize>,
    // Bubble-based chat rendering (replaces single text_view)
    pub bubble_container: Option<usize>, // NSStackView for bubbles
    pub bubble_views: Vec<(usize, usize)>, // (container, text_label) per message
    pub status_field: Option<usize>,
    pub input_field: Option<usize>,
    pub send_button: Option<usize>,
    pub attach_button: Option<usize>,
    pub auto_send_checkbox: Option<usize>,
    pub action_handler: Option<usize>,
    // Right panel (sidecar) for voice draft
    pub voice_draft_view: Option<usize>,
    pub voice_draft_header: Option<usize>,
    pub voice_send_button: Option<usize>,
    pub voice_use_button: Option<usize>,
    // Collapse button for sidecar
    pub collapse_button: Option<usize>,
    pub sidecar_collapsed: bool,
    // Right panel tab bar
    pub tab_bar: Option<usize>,
    pub selected_tab: usize, // 0 = Drafts, 1 = Settings
    // Drafts list (right panel content, tab 0)
    pub drafts_scroll_view: Option<usize>,
    pub drafts_container: Option<usize>, // NSStackView for draft rows
    pub draft_editor_scroll_view: Option<usize>,
    pub draft_editor_view: Option<usize>,
    pub draft_edit_button: Option<usize>,
    pub draft_copy_button: Option<usize>,
    pub draft_files: Vec<std::path::PathBuf>, // Cached list of draft files
    pub selected_draft_index: Option<usize>,
    pub editing_draft_index: Option<usize>,
    // Settings panel (right panel content, tab 1)
    pub settings_scroll_view: Option<usize>,
    pub settings_container: Option<usize>, // NSStackView for settings items
    pub ai_formatting_checkbox: Option<usize>,
    pub edit_buttons_container: Option<usize>, // Edit Config, Edit Prompt buttons
    // Chat state
    pub messages: Vec<ChatMessage>,
    // Separated buffers: manual input (left) vs voice streaming (right)
    pub manual_draft: String,
    pub voice_draft: String,
    // Attachments for manual input
    pub attachments: Vec<std::path::PathBuf>,
    // State flags
    pub is_sending: bool,
    pub auto_send_enabled: bool,
    pub is_voice_active: bool,
}

impl Default for VoiceChatOverlayState {
    fn default() -> Self {
        Self {
            window: None,
            window_delegate: None,
            scroll_view: None,
            bubble_container: None,
            bubble_views: Vec::new(),
            status_field: None,
            input_field: None,
            send_button: None,
            attach_button: None,
            auto_send_checkbox: None,
            action_handler: None,
            voice_draft_view: None,
            voice_draft_header: None,
            voice_send_button: None,
            voice_use_button: None,
            collapse_button: None,
            sidecar_collapsed: false,
            tab_bar: None,
            selected_tab: 0,
            drafts_scroll_view: None,
            drafts_container: None,
            draft_editor_scroll_view: None,
            draft_editor_view: None,
            draft_edit_button: None,
            draft_copy_button: None,
            draft_files: Vec::new(),
            selected_draft_index: None,
            editing_draft_index: None,
            settings_scroll_view: None,
            settings_container: None,
            ai_formatting_checkbox: None,
            edit_buttons_container: None,
            messages: Vec::new(),
            manual_draft: String::new(),
            voice_draft: String::new(),
            attachments: Vec::new(),
            is_sending: false,
            auto_send_enabled: true,
            is_voice_active: false,
        }
    }
}

lazy_static::lazy_static! {
    pub static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState::default());
    pub static ref SEND_CALLBACK: Mutex<Option<VoiceChatSendCallback>> = Mutex::new(None);
}
