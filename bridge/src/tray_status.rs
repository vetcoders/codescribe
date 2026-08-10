//! Live tray-status bridge.
//!
//! The core app crate owns `TrayStatus` and the producer-facing
//! `update_tray_status` API. This UniFFI slice registers a plain callback sink
//! into that core API, converts status changes into bridge-safe payloads, and
//! pushes them to the Swift menu-bar listener.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use codescribe::os::hold_badge::BadgeMode;
use codescribe::os::tray_status::{self, TrayStatus, TrayStatusSnapshot};
use tracing::trace;

/// Bridge-safe tray status kind.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsTrayStatusKind {
    Starting,
    Idle,
    Listening,
    Processing,
    Success,
    Error,
    Thermal,
    HotkeyConflict,
}

/// Coarse display tone for the Swift menu row.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsTrayStatusTone {
    Neutral,
    Active,
    Success,
    Warning,
    Critical,
}

/// Canonical recording semantics shared by badge, tray, and overlay.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsIndicatorMode {
    Hold,
    Toggle,
    Processing,
    Assistive,
}

impl From<BadgeMode> for CsIndicatorMode {
    fn from(value: BadgeMode) -> Self {
        match value {
            BadgeMode::Hold => Self::Hold,
            BadgeMode::Toggle => Self::Toggle,
            BadgeMode::Processing => Self::Processing,
            BadgeMode::Assistive => Self::Assistive,
        }
    }
}

/// One menu-bar status update.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsTrayStatusPayload {
    pub kind: CsTrayStatusKind,
    pub tone: CsTrayStatusTone,
    pub indicator_mode: CsIndicatorMode,
    pub assistive: bool,
    pub tooltip: String,
    pub menu_label: String,
    pub generation: u64,
}

#[uniffi::export(with_foreign)]
pub trait CsTrayStatusListener: Send + Sync {
    fn on_tray_status(&self, status: CsTrayStatusPayload);
}

#[derive(uniffi::Object, Default)]
pub struct CodescribeTrayStatus {}

#[uniffi::export]
impl CodescribeTrayStatus {
    #[uniffi::constructor]
    pub fn new() -> Self {
        codescribe::logging::init_logging();
        install_sink();
        Self::default()
    }

    /// Register or replace the Swift listener, then immediately seed it with the
    /// latest core-side status so the menu never starts stale.
    pub fn set_listener(&self, listener: Arc<dyn CsTrayStatusListener>) {
        install_sink();
        {
            let listener_store = shared_listener();
            let mut guard = listener_store
                .write()
                .unwrap_or_else(|error| error.into_inner());
            *guard = Some(Arc::clone(&listener));
        }
        listener.on_tray_status(current_payload());
    }

    /// Current status snapshot for Swift surfaces that need an initial value.
    pub fn current_status(&self) -> CsTrayStatusPayload {
        install_sink();
        current_payload()
    }
}

type SharedListener = RwLock<Option<Arc<dyn CsTrayStatusListener>>>;

fn shared_listener() -> &'static SharedListener {
    static LISTENER: OnceLock<SharedListener> = OnceLock::new();
    LISTENER.get_or_init(|| RwLock::new(None))
}

fn last_forwarded_status() -> &'static Mutex<Option<TrayStatusSnapshot>> {
    static LAST_FORWARDED: OnceLock<Mutex<Option<TrayStatusSnapshot>>> = OnceLock::new();
    LAST_FORWARDED.get_or_init(|| Mutex::new(None))
}

fn generation_counter() -> &'static AtomicU64 {
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    &GENERATION
}

fn install_sink() {
    tray_status::set_tray_status_sink(Some(Arc::new(publish_tray_status)));
}

fn current_payload() -> CsTrayStatusPayload {
    payload_from_status(
        tray_status::current_tray_status_snapshot(),
        generation_counter().load(Ordering::SeqCst),
    )
}

fn payload_from_status(snapshot: TrayStatusSnapshot, generation: u64) -> CsTrayStatusPayload {
    let status = snapshot.status;
    let (kind, tone) = match status {
        TrayStatus::Starting => (CsTrayStatusKind::Starting, CsTrayStatusTone::Neutral),
        TrayStatus::Idle => (CsTrayStatusKind::Idle, CsTrayStatusTone::Neutral),
        TrayStatus::Listening => (CsTrayStatusKind::Listening, CsTrayStatusTone::Active),
        TrayStatus::Thinking => (CsTrayStatusKind::Processing, CsTrayStatusTone::Active),
        TrayStatus::Success => (CsTrayStatusKind::Success, CsTrayStatusTone::Success),
        TrayStatus::Error => (CsTrayStatusKind::Error, CsTrayStatusTone::Critical),
        TrayStatus::Thermal => (CsTrayStatusKind::Thermal, CsTrayStatusTone::Warning),
        TrayStatus::HotkeyConflict => (CsTrayStatusKind::HotkeyConflict, CsTrayStatusTone::Warning),
    };
    CsTrayStatusPayload {
        kind,
        tone,
        indicator_mode: snapshot.indicator_mode.into(),
        assistive: snapshot.is_assistive_visible(),
        tooltip: snapshot.tooltip(),
        menu_label: snapshot.menu_label().to_string(),
        generation,
    }
}

fn publish_tray_status(snapshot: TrayStatusSnapshot) {
    let changed = {
        let mut last = last_forwarded_status()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *last == Some(snapshot) {
            false
        } else {
            *last = Some(snapshot);
            true
        }
    };
    if !changed {
        trace!(
            status = ?snapshot.status,
            indicator_mode = ?snapshot.indicator_mode,
            "coalesced duplicate tray status"
        );
        return;
    }

    let generation = generation_counter().fetch_add(1, Ordering::SeqCst) + 1;
    let payload = payload_from_status(snapshot, generation);
    let listener_store = shared_listener();
    let listener = listener_store
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(Arc::clone);

    if let Some(listener) = listener {
        listener.on_tray_status(payload);
    } else {
        trace!(
            status = ?snapshot.status,
            indicator_mode = ?snapshot.indicator_mode,
            "tray status changed without Swift listener"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    struct CapturingTrayStatusListener {
        calls: Arc<StdMutex<Vec<CsTrayStatusPayload>>>,
    }

    impl CsTrayStatusListener for CapturingTrayStatusListener {
        fn on_tray_status(&self, status: CsTrayStatusPayload) {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(status);
        }
    }

    fn reset_for_test() {
        tray_status::set_tray_status_sink(None);
        tray_status::set_tray_assistive_session(false);
        tray_status::update_tray_status(TrayStatus::Idle);
        let listener_store = shared_listener();
        *listener_store
            .write()
            .unwrap_or_else(|error| error.into_inner()) = None;
        *last_forwarded_status()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        generation_counter().store(0, Ordering::SeqCst);
    }

    #[test]
    fn maps_core_status_to_bridge_payload() {
        let payload = payload_from_status(TrayStatusSnapshot::new(TrayStatus::Thinking, false), 42);

        assert_eq!(payload.kind, CsTrayStatusKind::Processing);
        assert_eq!(payload.tone, CsTrayStatusTone::Active);
        assert_eq!(payload.indicator_mode, CsIndicatorMode::Processing);
        assert!(!payload.assistive);
        assert_eq!(payload.tooltip, "Codescribe - Processing...");
        assert_eq!(payload.menu_label, "Status: Processing...");
        assert_eq!(payload.generation, 42);
    }

    #[test]
    fn maps_assistive_status_to_agent_payload_copy() {
        let payload = payload_from_status(TrayStatusSnapshot::new(TrayStatus::Listening, true), 43);

        assert_eq!(payload.kind, CsTrayStatusKind::Listening);
        assert_eq!(payload.tone, CsTrayStatusTone::Active);
        assert_eq!(payload.indicator_mode, CsIndicatorMode::Assistive);
        assert!(payload.assistive);
        assert_eq!(payload.tooltip, "Codescribe - Agent listening...");
        assert_eq!(payload.menu_label, "Status: Agent listening...");
        assert_eq!(payload.generation, 43);
    }

    #[test]
    #[serial_test::serial]
    fn listener_receives_changes_and_coalesces_duplicates() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        reset_for_test();

        let tray_status_bridge = CodescribeTrayStatus::new();
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let listener = Arc::new(CapturingTrayStatusListener {
            calls: Arc::clone(&calls),
        });
        tray_status_bridge.set_listener(listener);
        calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();

        tray_status::update_tray_status(TrayStatus::Thermal);
        tray_status::update_tray_status(TrayStatus::Thermal);
        tray_status::update_tray_status(TrayStatus::HotkeyConflict);

        let calls = calls.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].kind, CsTrayStatusKind::Thermal);
        assert_eq!(calls[0].tone, CsTrayStatusTone::Warning);
        assert_eq!(calls[1].kind, CsTrayStatusKind::HotkeyConflict);
        assert!(calls[1].generation > calls[0].generation);
    }

    #[test]
    #[serial_test::serial]
    fn listener_refires_when_only_assistive_lane_changes() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        reset_for_test();

        let tray_status_bridge = CodescribeTrayStatus::new();
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let listener = Arc::new(CapturingTrayStatusListener {
            calls: Arc::clone(&calls),
        });
        tray_status_bridge.set_listener(listener);
        calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();

        tray_status::update_tray_status(TrayStatus::Listening);
        tray_status::update_tray_status(TrayStatus::Listening);
        tray_status::set_tray_assistive_session(true);
        tray_status::set_tray_assistive_session(true);

        let calls = calls.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].kind, CsTrayStatusKind::Listening);
        assert!(!calls[0].assistive);
        assert_eq!(calls[1].kind, CsTrayStatusKind::Listening);
        assert!(calls[1].assistive);
        assert!(calls[1].generation > calls[0].generation);
    }
}
