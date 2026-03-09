use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSignal {
    Thinking,
    Error,
}

pub type StatusCallback = Arc<dyn Fn(StatusSignal) + Send + Sync>;

static STATUS_CALLBACK: OnceLock<StatusCallback> = OnceLock::new();

pub fn set_status_callback(callback: StatusCallback) {
    let _ = STATUS_CALLBACK.set(callback);
}

pub fn notify_status(status: StatusSignal) {
    if let Some(callback) = STATUS_CALLBACK.get() {
        (callback)(status);
    }
}
