//! Voice Chat UI state and types
//!
//! Contains overlay state, configuration, and message types.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use codescribe_core::attachment::Attachment;

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
    /// Live reasoning summary streamed from the agent (its own lane, rendered
    /// as a collapsible "thinking" entry — NOT mixed into the assistant text).
    Reasoning,
}

/// A single chat message
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
    pub is_streaming: bool,
    pub is_collapsed: bool,
    pub is_error: bool,
    pub timestamp: SystemTime,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Drawer,
    Agent,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawerEntrySource {
    LegacyFile,
    Thread { id: String },
}

pub struct DrawerEntry {
    pub source: DrawerEntrySource,
    pub path: PathBuf,
    pub timestamp: SystemTime,
    pub mode: TranscriptionMode,
    pub preview: String,
    pub search_corpus: String,
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
    pub tab_agent_button: Option<usize>,
    pub tab_settings_button: Option<usize>,
    pub favorites_button: Option<usize>,
    pub help_button: Option<usize>,
    pub close_button: Option<usize>,

    // Drawer tab
    pub drawer_scroll_view: Option<usize>,
    pub drawer_container: Option<usize>,
    pub drawer_entries: Vec<DrawerEntry>,
    pub drawer_edge_effect: Option<usize>,
    pub search_field: Option<usize>,
    pub search_label: Option<usize>,
    pub drawer_favorites_only: bool,
    pub favorites: HashSet<String>,

    // Agent tab
    pub agent_scroll_view: Option<usize>,
    pub agent_container: Option<usize>,
    pub agent_bubble_views: Vec<(usize, usize)>,
    pub agent_bubble_click_recognizers: Vec<(usize, usize)>,
    pub agent_input_bar: Option<usize>,
    pub agent_input_scroll_view: Option<usize>,
    pub agent_input_text_view: Option<usize>,
    pub agent_input_field: Option<usize>,
    pub agent_attach_button: Option<usize>,
    pub agent_send_button: Option<usize>,
    pub agent_latest_button: Option<usize>,
    /// Attachments (files, images, URLs, GitHub blobs) for Agent chat context.
    pub attachments: Vec<Attachment>,
    /// Fingerprint of the last attachment set sent to the assistant.
    pub attachments_last_sent: Option<u64>,
    /// Chip strip scroll view (horizontal list of attachment chips above input bar).
    pub attachment_chip_strip: Option<usize>,

    // Active tab
    pub active_tab: Tab,
    /// Requested tab to apply after overlay is created.
    pub pending_tab: Option<Tab>,

    // Chat state
    pub messages: Vec<ChatMessage>,
    /// Active streaming user message index (if any).
    pub active_user_stream_index: Option<usize>,
    /// Active streaming assistant message index (if any).
    pub active_assistant_stream_index: Option<usize>,
    /// Active streaming reasoning-summary message index (if any).
    pub active_reasoning_stream_index: Option<usize>,
    pub manual_draft: String,
    pub is_sending: bool,
    /// True while the agent is reasoning after a voice transcript was handed off / sent.
    /// Used to show "Thinking..." / reasoning indicator in the Agent tab.
    pub is_agent_thinking: bool,
    /// True when the user is pinned to the bottom of the agent transcript.
    pub scroll_pinned: bool,
    pub auto_send_enabled: bool,
    /// Last status text provided by caller before runtime-health decoration is applied.
    pub status_base_text: String,
    pub status_text: String,
    pub status_kind: UiStatus,
    pub context_text: String,
    /// True when agent runtime is unavailable and legacy fallback is active.
    pub runtime_degraded: bool,
    /// Explicit UI flag for showing persistent "fallback active" indicators.
    pub is_agent_degraded: bool,
    /// Optional diagnostic context for degraded runtime state.
    pub runtime_degraded_reason: Option<String>,
    /// Best-effort app name to reactivate when performing paste actions.
    pub last_target_app: Option<String>,

    // Conversation mode (Moshi)
    pub conversation_state: ConversationModeState,

    // Handler
    pub action_handler: Option<usize>,

    // Throttling: last time we ran a layout pass for streaming deltas
    pub last_layout_time: Option<Instant>,
    pub layout_pending: bool,
    pub pending_delta_index: Option<usize>,

    // Zoom
    pub zoom_level: f64,
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
            tab_agent_button: None,
            tab_settings_button: None,
            favorites_button: None,
            help_button: None,
            close_button: None,
            drawer_scroll_view: None,
            drawer_container: None,
            drawer_entries: Vec::new(),
            drawer_edge_effect: None,
            search_field: None,
            search_label: None,
            drawer_favorites_only: false,
            favorites: HashSet::new(),
            agent_scroll_view: None,
            agent_container: None,
            agent_bubble_views: Vec::new(),
            agent_bubble_click_recognizers: Vec::new(),
            agent_input_bar: None,
            agent_input_scroll_view: None,
            agent_input_text_view: None,
            agent_input_field: None,
            agent_attach_button: None,
            agent_send_button: None,
            agent_latest_button: None,
            attachments: Vec::new(),
            attachments_last_sent: None,
            attachment_chip_strip: None,
            active_tab: Tab::Drawer,
            pending_tab: None,
            messages: Vec::new(),
            active_user_stream_index: None,
            active_assistant_stream_index: None,
            active_reasoning_stream_index: None,
            manual_draft: String::new(),
            is_sending: false,
            is_agent_thinking: false,
            scroll_pinned: true,
            auto_send_enabled: true,
            status_base_text: "Ready".to_string(),
            status_text: "Ready".to_string(),
            status_kind: UiStatus::Idle,
            context_text: String::new(),
            runtime_degraded: false,
            is_agent_degraded: false,
            runtime_degraded_reason: None,
            last_target_app: None,
            conversation_state: ConversationModeState::default(),
            action_handler: None,
            last_layout_time: None,
            layout_pending: false,
            pending_delta_index: None,
            zoom_level: 1.0,
        }
    }
}

lazy_static::lazy_static! {
    pub static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState::default());
    pub static ref SEND_CALLBACK: Mutex<Option<VoiceChatSendCallback>> = Mutex::new(None);
}
