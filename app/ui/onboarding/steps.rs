//! Step metadata for first-run onboarding wizard.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    Microphone,
    Accessibility,
    InputMonitoring,
    ScreenRecording,
    FullDiskAccess,
}

impl PermissionKind {
    pub const fn index(self) -> usize {
        match self {
            Self::Microphone => 0,
            Self::Accessibility => 1,
            Self::InputMonitoring => 2,
            Self::ScreenRecording => 3,
            Self::FullDiskAccess => 4,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone Access",
            Self::Accessibility => "Accessibility Access",
            Self::InputMonitoring => "Input Monitoring Access",
            Self::ScreenRecording => "Screen Recording Access",
            Self::FullDiskAccess => "Full Disk Access",
        }
    }

    pub const fn reason(self) -> &'static str {
        match self {
            Self::Microphone => {
                "Transcribe your voice into text. Audio is processed locally on your Mac."
            }
            Self::Accessibility => {
                "Type transcribed text into any application and control text insertion."
            }
            Self::InputMonitoring => "Detect keyboard shortcuts to start and stop voice recording.",
            Self::ScreenRecording => {
                "Capture screen context to give the AI assistant visual awareness of what you're working on."
            }
            Self::FullDiskAccess => {
                "Read project files for AI context. Optional - limits file-aware features if skipped."
            }
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Microphone => "MIC",
            Self::Accessibility => "AX",
            Self::InputMonitoring => "KEY",
            Self::ScreenRecording => "SCR",
            Self::FullDiskAccess => "FILE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    Permission(PermissionKind),
    Language,
    ApiKey,
    HotkeyMode,
    Done,
}

pub const TOTAL_STEPS: usize = 10;

pub const STEP_FLOW: [WizardStep; TOTAL_STEPS] = [
    WizardStep::Welcome,
    WizardStep::Permission(PermissionKind::Microphone),
    WizardStep::Permission(PermissionKind::Accessibility),
    WizardStep::Permission(PermissionKind::InputMonitoring),
    WizardStep::Permission(PermissionKind::ScreenRecording),
    WizardStep::Permission(PermissionKind::FullDiskAccess),
    WizardStep::Language,
    WizardStep::ApiKey,
    WizardStep::HotkeyMode,
    WizardStep::Done,
];

pub fn step_for_index(index: usize) -> WizardStep {
    STEP_FLOW.get(index).copied().unwrap_or(WizardStep::Welcome)
}
