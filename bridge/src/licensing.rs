//! Thin UniFFI projection of the canonical Rust licensing verifier.

use chrono::{DateTime, Utc};
use codescribe_core::licensing::{LicensePolicy, LicenseState, LicenseStatus, evaluate_license};

use crate::CsError;

#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsLicenseState {
    Unlicensed,
    Active,
    GraceOffline,
    ExpiredUpdates,
}

#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct CsLicenseStatus {
    pub state: CsLicenseState,
    pub days_left: Option<u8>,
    pub sku: Option<String>,
    pub email_hash: Option<String>,
    pub issued: Option<String>,
    pub updates_until: Option<String>,
    pub seat_limit: Option<u16>,
    pub agentic_entitled: bool,
}

impl From<LicenseStatus> for CsLicenseStatus {
    fn from(status: LicenseStatus) -> Self {
        let (state, days_left) = match status.state {
            LicenseState::Unlicensed => (CsLicenseState::Unlicensed, None),
            LicenseState::Active => (CsLicenseState::Active, None),
            LicenseState::GraceOffline { days_left } => {
                (CsLicenseState::GraceOffline, Some(days_left))
            }
            LicenseState::ExpiredUpdates => (CsLicenseState::ExpiredUpdates, None),
        };
        let agentic_entitled = status.allows_agentic(&LicensePolicy::default());
        let claims = status.claims;
        Self {
            state,
            days_left,
            sku: claims.as_ref().map(|claims| claims.sku.clone()),
            email_hash: claims.as_ref().map(|claims| claims.email_hash.clone()),
            issued: claims.as_ref().map(|claims| claims.issued.to_string()),
            updates_until: claims
                .as_ref()
                .map(|claims| claims.updates_until.to_string()),
            seat_limit: claims.as_ref().map(|claims| claims.seat_limit),
            agentic_entitled,
        }
    }
}

/// Validate a newly entered signed key. The successful explicit validation is
/// Active and its timestamp is persisted by `LicenseService` with the key.
#[uniffi::export]
pub fn license_activate(key: String, now_unix_seconds: i64) -> Result<CsLicenseStatus, CsError> {
    let now = timestamp(now_unix_seconds)?;
    evaluate_license(Some(&key), now, Some(now), true)
        .map(Into::into)
        .map_err(license_error)
}

/// Evaluate persisted local truth at launch. Absence is an honest Unlicensed
/// state; malformed/tampered persisted data fails closed across the bridge.
#[uniffi::export]
pub fn license_status(
    key: Option<String>,
    last_online_validation_unix_seconds: Option<i64>,
    now_unix_seconds: i64,
) -> Result<CsLicenseStatus, CsError> {
    let last_online = last_online_validation_unix_seconds
        .map(timestamp)
        .transpose()?;
    evaluate_license(
        key.as_deref(),
        timestamp(now_unix_seconds)?,
        last_online,
        false,
    )
    .map(Into::into)
    .map_err(license_error)
}

fn timestamp(seconds: i64) -> Result<DateTime<Utc>, CsError> {
    DateTime::from_timestamp(seconds, 0).ok_or_else(|| CsError::License {
        msg: "License timestamp is outside the supported range".to_string(),
    })
}

fn license_error(error: codescribe_core::licensing::LicenseError) -> CsError {
    CsError::License {
        msg: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licensing_absent_key_crosses_bridge_as_unlicensed() {
        let status = license_status(None, None, 1_775_304_000).unwrap();
        assert_eq!(status.state, CsLicenseState::Unlicensed);
        assert!(!status.agentic_entitled);
    }
}
