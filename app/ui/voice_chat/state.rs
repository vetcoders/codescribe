//! Voice Chat UI state and types
//!
//! Contains overlay state, configuration, and message types.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use crate::ui::shared::status::UiStatus;

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
            width: 450.0,
            height: 520.0,
            auto_hide_timeout_secs: 0,
        }
    }
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
    pub timestamp: SystemTime,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Drawer,
    Transcription,
    Agent,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptionMode {
    Hold,
    Assistive,
    Toggle,
    /// Full-duplex conversation mode (Moshi)
    Conversation,
}

/// State of the conversation mode (Moshi full-duplex)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversationModeState {
    /// Not in conversation mode
    #[default]
    Inactive,
    /// Listening for user speech
    Listening,
    /// User is speaking
    UserSpeaking,
    /// Processing user input
    Processing,
    /// Assistant is responding (audio playing)
    AssistantSpeaking,
    /// User interrupted assistant
    Interrupted,
}

pub struct DrawerEntry {
    pub path: PathBuf,
    pub timestamp: SystemTime,
    pub mode: TranscriptionMode,
    pub preview: String,
    pub is_ai_formatted: bool,
    pub is_favorite: bool,
}

/// Voice chat overlay state
pub struct VoiceChatOverlayState {
    // Window
    pub window: Option<usize>,
    pub window_delegate: Option<usize>,
    pub blur_view: Option<usize>,
    pub split_view_controller: Option<usize>,
    pub split_sidebar_item: Option<usize>,
    pub split_content_item: Option<usize>,
    pub split_sidebar_container: Option<usize>,
    pub split_content_container: Option<usize>,

    // Header
    pub title_label: Option<usize>,
    pub status_pill: Option<usize>,
    pub status_pill_label: Option<usize>,
    pub status_pill_dot: Option<usize>,
    pub tab_drawer_button: Option<usize>,
    pub tab_transcription_button: Option<usize>,
    pub tab_agent_button: Option<usize>,
    pub tab_settings_button: Option<usize>,
    pub favorites_button: Option<usize>,
    pub close_button: Option<usize>,
    pub settings_view: Option<usize>,

    // Drawer tab
    pub drawer_scroll_view: Option<usize>,
    pub drawer_container: Option<usize>,
    pub drawer_entries: Vec<DrawerEntry>,
    pub drawer_edge_effect: Option<usize>,
    pub search_field: Option<usize>,
    pub search_label: Option<usize>,
    pub help_panel: Option<usize>,
    pub help_hold_label: Option<usize>,
    pub help_toggle_label: Option<usize>,
    pub drawer_favorites_only: bool,
    pub favorites: HashSet<String>,

    // Agent tab
    pub agent_scroll_view: Option<usize>,
    pub agent_container: Option<usize>,
    pub agent_bubble_views: Vec<(usize, usize)>,
    pub agent_input_bar: Option<usize>,
    pub agent_input_scroll_view: Option<usize>,
    pub agent_input_text_view: Option<usize>,
    pub agent_input_field: Option<usize>,
    pub agent_attach_button: Option<usize>,
    pub agent_send_button: Option<usize>,
    /// Files attached as additional context for Agent chat.
    pub attached_files: Vec<PathBuf>,
    /// Fingerprint of the last attachment set that was sent to the assistant.
    pub attached_files_last_sent: Option<u64>,

    // Transcription tab (one-overlay mode)
    pub transcription_scroll_view: Option<usize>,
    pub transcription_text_view: Option<usize>,
    pub transcription_placeholder: Option<usize>,
    pub transcription_edge_effect: Option<usize>,
    pub transcription_text: String,

    // Active tab
    pub active_tab: Tab,

    // Chat state
    pub messages: Vec<ChatMessage>,
    pub manual_draft: String,
    pub is_sending: bool,
    pub auto_send_enabled: bool,
    pub status_text: String,
    pub status_kind: UiStatus,
    pub context_text: String,
    /// Best-effort app name to reactivate when performing paste actions.
    pub last_target_app: Option<String>,

    // Conversation mode (Moshi)
    pub conversation_state: ConversationModeState,

    // Handler
    pub action_handler: Option<usize>,

    // Throttling: last time we ran a layout pass for streaming deltas
    pub last_layout_time: Option<Instant>,
    pub layout_pending: bool,
}

impl Default for VoiceChatOverlayState {
    fn default() -> Self {
        Self {
            window: None,
            window_delegate: None,
            blur_view: None,
            split_view_controller: None,
            split_sidebar_item: None,
            split_content_item: None,
            split_sidebar_container: None,
            split_content_container: None,
            title_label: None,
            status_pill: None,
            status_pill_label: None,
            status_pill_dot: None,
            tab_drawer_button: None,
            tab_transcription_button: None,
            tab_agent_button: None,
            tab_settings_button: None,
            favorites_button: None,
            close_button: None,
            settings_view: None,
            drawer_scroll_view: None,
            drawer_container: None,
            drawer_entries: Vec::new(),
            drawer_edge_effect: None,
            search_field: None,
            search_label: None,
            help_panel: None,
            help_hold_label: None,
            help_toggle_label: None,
            drawer_favorites_only: false,
            favorites: HashSet::new(),
            agent_scroll_view: None,
            agent_container: None,
            agent_bubble_views: Vec::new(),
            agent_input_bar: None,
            agent_input_scroll_view: None,
            agent_input_text_view: None,
            agent_input_field: None,
            agent_attach_button: None,
            agent_send_button: None,
            attached_files: Vec::new(),
            attached_files_last_sent: None,
            transcription_scroll_view: None,
            transcription_text_view: None,
            transcription_placeholder: None,
            transcription_edge_effect: None,
            transcription_text: String::new(),
            active_tab: Tab::Drawer,
            messages: Vec::new(),
            manual_draft: String::new(),
            is_sending: false,
            auto_send_enabled: true,
            status_text: "Ready".to_string(),
            status_kind: UiStatus::Idle,
            context_text: String::new(),
            last_target_app: None,
            conversation_state: ConversationModeState::default(),
            action_handler: None,
            last_layout_time: None,
            layout_pending: false,
        }
    }
}

lazy_static::lazy_static! {
    pub static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState::default());
    pub static ref SEND_CALLBACK: Mutex<Option<VoiceChatSendCallback>> = Mutex::new(None);
}
