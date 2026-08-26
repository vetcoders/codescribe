//! Canonical process thermal-pressure state.
//!
//! Platform adapters publish observed device pressure here. The value is
//! intentionally descriptive only: it does not dispatch work, pause lanes, or
//! own any scheduler policy.

use std::sync::atomic::{AtomicU8, Ordering};

/// Process thermal pressure reported by the host operating system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThermalLevel {
    /// The device is operating without thermal pressure.
    #[default]
    Nominal,
    /// The device reports elevated but non-serious pressure.
    Fair,
    /// The device reports serious thermal pressure.
    Serious,
    /// The device reports critical thermal pressure.
    Critical,
}

impl ThermalLevel {
    const fn as_u8(self) -> u8 {
        match self {
            Self::Nominal => 0,
            Self::Fair => 1,
            Self::Serious => 2,
            Self::Critical => 3,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Fair,
            2 => Self::Serious,
            3 => Self::Critical,
            _ => Self::Nominal,
        }
    }
}

/// The last thermal level published for this process.
static PROCESS_THERMAL_LEVEL: AtomicU8 = AtomicU8::new(ThermalLevel::Nominal.as_u8());

/// Return the last thermal level published by the platform observer.
pub fn current_process_thermal_level() -> ThermalLevel {
    ThermalLevel::from_u8(PROCESS_THERMAL_LEVEL.load(Ordering::Relaxed))
}

/// Replace the canonical process thermal level.
pub fn set_process_thermal_level(level: ThermalLevel) {
    PROCESS_THERMAL_LEVEL.store(level.as_u8(), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every macOS pressure transition survives without a scheduler or registry.
    #[test]
    #[serial_test::serial]
    fn process_state_roundtrips_all_thermal_transitions() {
        let original = current_process_thermal_level();
        for expected in [
            ThermalLevel::Nominal,
            ThermalLevel::Fair,
            ThermalLevel::Serious,
            ThermalLevel::Critical,
            ThermalLevel::Nominal,
        ] {
            set_process_thermal_level(expected);
            assert_eq!(current_process_thermal_level(), expected);
        }
        set_process_thermal_level(original);
    }
}
