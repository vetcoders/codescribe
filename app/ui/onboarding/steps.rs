//! Step metadata for first-run onboarding wizard.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionRecoveryStrategy {
    LiveRecheck,
    LiveReinitialize,
    AppRestartRequired,
}

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

    pub const fn runtime_subsystem(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone capture",
            Self::Accessibility | Self::InputMonitoring => "Global hotkeys",
            Self::ScreenRecording => "Screen capture",
            Self::FullDiskAccess => "Protected file access",
        }
    }

    pub const fn recovery_strategy(self) -> PermissionRecoveryStrategy {
        match self {
            Self::Microphone => PermissionRecoveryStrategy::LiveRecheck,
            Self::Accessibility | Self::InputMonitoring => {
                PermissionRecoveryStrategy::LiveReinitialize
            }
            Self::ScreenRecording | Self::FullDiskAccess => {
                PermissionRecoveryStrategy::AppRestartRequired
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    /// First-run operating-lane choice (Basic vs Agentic). Placed right after
    /// Welcome so the lane framing is set before the rest of the wizard.
    Mode,
    Permission(PermissionKind),
    Language,
    ApiKey,
    HotkeyMode,
    /// Agentic-only readiness verdict (Vibecrafted / AICX / Loctree / PRView).
    /// Present in the fixed flow but navigated *around* in the Basic lane — see
    /// `actions::step_is_visible`. Keeping it in the array (rather than a
    /// mode-dependent flow) preserves stable, resume-safe step indices.
    AgenticReadiness,
    Done,
}

pub const STEP_FLOW: [WizardStep; 12] = [
    WizardStep::Welcome,
    WizardStep::Mode,
    WizardStep::Permission(PermissionKind::Microphone),
    WizardStep::Permission(PermissionKind::Accessibility),
    WizardStep::Permission(PermissionKind::InputMonitoring),
    WizardStep::Permission(PermissionKind::ScreenRecording),
    WizardStep::Permission(PermissionKind::FullDiskAccess),
    WizardStep::Language,
    WizardStep::ApiKey,
    WizardStep::HotkeyMode,
    WizardStep::AgenticReadiness,
    WizardStep::Done,
];

pub const TOTAL_STEPS: usize = STEP_FLOW.len();

pub fn step_for_index(index: usize) -> WizardStep {
    STEP_FLOW.get(index).copied().unwrap_or(WizardStep::Welcome)
}
