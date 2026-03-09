use objc::{sel, sel_impl};

use crate::tray::TrayStatus;

type Id = *mut objc::runtime::Object;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiStatus {
    Idle,
    Listening,
    Processing,
    Error,
}

pub struct StatusPalette {
    pub bg: (f64, f64, f64, f64),
    pub text: (f64, f64, f64, f64),
    pub dot: (f64, f64, f64, f64),
}

impl UiStatus {
    pub fn label(self) -> &'static str {
        match self {
            UiStatus::Idle => "Idle",
            UiStatus::Listening => "Listening",
            UiStatus::Processing => "Processing",
            UiStatus::Error => "Error",
        }
    }

    pub fn palette(self) -> StatusPalette {
        match self {
            UiStatus::Idle => StatusPalette {
                bg: (0.12, 0.2, 0.14, 0.65),
                text: (0.78, 0.95, 0.82, 1.0),
                dot: (0.36, 0.92, 0.55, 1.0),
            },
            UiStatus::Listening => StatusPalette {
                bg: (0.22, 0.12, 0.12, 0.7),
                text: (0.98, 0.78, 0.78, 1.0),
                dot: (0.98, 0.35, 0.35, 1.0),
            },
            UiStatus::Processing => StatusPalette {
                bg: (0.24, 0.18, 0.1, 0.75),
                text: (0.98, 0.88, 0.7, 1.0),
                dot: (0.98, 0.7, 0.25, 1.0),
            },
            UiStatus::Error => StatusPalette {
                bg: (0.28, 0.1, 0.1, 0.8),
                text: (1.0, 0.75, 0.75, 1.0),
                dot: (1.0, 0.25, 0.25, 1.0),
            },
        }
    }

    /// System-dynamic text color for status labels.
    /// Uses NSColor named colors that adapt to light/dark appearance.
    pub fn text_color(self) -> Id {
        unsafe {
            let cls = objc::runtime::Class::get("NSColor").unwrap();
            match self {
                UiStatus::Idle => objc::msg_send![cls, systemGreenColor],
                UiStatus::Listening => objc::msg_send![cls, systemRedColor],
                UiStatus::Processing => objc::msg_send![cls, systemOrangeColor],
                UiStatus::Error => objc::msg_send![cls, systemRedColor],
            }
        }
    }

    pub fn to_tray(self) -> TrayStatus {
        match self {
            UiStatus::Idle => TrayStatus::Idle,
            UiStatus::Listening => TrayStatus::Listening,
            UiStatus::Processing => TrayStatus::Thinking,
            UiStatus::Error => TrayStatus::Error,
        }
    }
}

pub fn status_from_detail(detail: &str) -> UiStatus {
    let text = detail.trim().to_lowercase();
    if text.is_empty() {
        return UiStatus::Idle;
    }

    if text.contains("error")
        || text.contains("failed")
        || text.contains("błąd")
        || text.contains("unavailable")
    {
        return UiStatus::Error;
    }

    if text.contains("processing")
        || text.contains("formatting")
        || text.contains("augment")
        || text.contains("finalizing")
        || text.contains("thinking")
        || text.contains("responding")
        || text.contains("sending")
        || text.contains("wysyłam")
        || text.contains("wysylam")
    {
        return UiStatus::Processing;
    }

    if text.contains("listening")
        || text.contains("recording")
        || text.contains("nagrywam")
        || text.contains("speaking")
    {
        return UiStatus::Listening;
    }

    if text.contains("ready")
        || text.contains("idle")
        || text.contains("ended")
        || text.contains("response")
        || text.contains("formatted")
        || text.contains("no selection")
    {
        return UiStatus::Idle;
    }

    UiStatus::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_from_detail_maps_key_phrases() {
        assert_eq!(status_from_detail("Listening..."), UiStatus::Listening);
        assert_eq!(status_from_detail("Processing..."), UiStatus::Processing);
        assert_eq!(status_from_detail("Augmenting..."), UiStatus::Processing);
        assert_eq!(status_from_detail("Formatting Failed"), UiStatus::Error);
        assert_eq!(status_from_detail("Conversation ended"), UiStatus::Idle);
        assert_eq!(status_from_detail("AI Response:"), UiStatus::Idle);
    }

    #[test]
    fn status_to_tray_mapping() {
        assert_eq!(UiStatus::Idle.to_tray(), TrayStatus::Idle);
        assert_eq!(UiStatus::Listening.to_tray(), TrayStatus::Listening);
        assert_eq!(UiStatus::Processing.to_tray(), TrayStatus::Thinking);
        assert_eq!(UiStatus::Error.to_tray(), TrayStatus::Error);
    }
}
