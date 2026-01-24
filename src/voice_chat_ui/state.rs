//! Voice Chat UI state and types
//!
//! Contains overlay state, configuration, and message types.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Type alias for voice chat send callback
pub type VoiceChatSendCallback = Arc<dyn Fn(String) + Send + Sync>;

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
}

#[derive(Debug, Clone)]
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

    // Header
    pub title_label: Option<usize>,
    pub tab_control: Option<usize>,
    pub close_button: Option<usize>,
    pub settings_button: Option<usize>,

    // Drawer tab
    pub drawer_scroll_view: Option<usize>,
    pub drawer_container: Option<usize>,
    pub drawer_entries: Vec<DrawerEntry>,
    pub search_field: Option<usize>,

    // Agent tab
    pub agent_scroll_view: Option<usize>,
    pub agent_container: Option<usize>,
    pub agent_bubble_views: Vec<(usize, usize)>,
    pub agent_input_field: Option<usize>,
    pub agent_send_button: Option<usize>,

    // Active tab
    pub active_tab: Tab,

    // Chat state (Agent tab)
    pub messages: Vec<ChatMessage>,
    pub manual_draft: String,
    pub is_sending: bool,
    pub auto_send_enabled: bool,

    // Handler
    pub action_handler: Option<usize>,
}

impl Default for VoiceChatOverlayState {
    fn default() -> Self {
        Self {
            window: None,
            window_delegate: None,
            blur_view: None,
            title_label: None,
            tab_control: None,
            close_button: None,
            settings_button: None,
            drawer_scroll_view: None,
            drawer_container: None,
            drawer_entries: Vec::new(),
            search_field: None,
            agent_scroll_view: None,
            agent_container: None,
            agent_bubble_views: Vec::new(),
            agent_input_field: None,
            agent_send_button: None,
            active_tab: Tab::Drawer,
            messages: Vec::new(),
            manual_draft: String::new(),
            is_sending: false,
            auto_send_enabled: true,
            action_handler: None,
        }
    }
}

lazy_static::lazy_static! {
    pub static ref OVERLAY_STATE: Mutex<VoiceChatOverlayState> = Mutex::new(VoiceChatOverlayState::default());
    pub static ref SEND_CALLBACK: Mutex<Option<VoiceChatSendCallback>> = Mutex::new(None);
}
