//! Local-first CSK1 license verification and entitlement state.
//!
//! The public verification key has one owner here. The checked-in key is the
//! development key used by tests and the `licensing-dev` helper. Before a
//! production release the operator replaces only `LICENSE_PUBLIC_KEY_BYTES`;
//! the production private key never enters this repository or the app bundle.

use std::collections::BTreeSet;
use std::fmt;

use base64::Engine as _;
use chrono::{DateTime, NaiveDate, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const LICENSE_PREFIX: &str = "CSK1";
pub const DEFAULT_AGENTIC_SKU: &str = "agentic-lifetime";
pub const OFFLINE_GRACE_DAYS: i64 = 30;

/// RFC 8032 test-vector public key. DEVELOPMENT ONLY; replace for production.
pub const LICENSE_PUBLIC_KEY_BYTES: [u8; 32] = [
    0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07, 0x3a,
    0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07, 0x51, 0x1a,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseClaims {
    pub v: u8,
    pub sku: String,
    pub email_hash: String,
    pub issued: NaiveDate,
    pub updates_until: NaiveDate,
    pub seat_limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseState {
    Unlicensed,
    Active,
    GraceOffline { days_left: u8 },
    ExpiredUpdates,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseStatus {
    pub state: LicenseState,
    pub claims: Option<LicenseClaims>,
}

impl LicenseStatus {
    pub fn unlicensed() -> Self {
        Self {
            state: LicenseState::Unlicensed,
            claims: None,
        }
    }

    pub fn allows_agentic(&self, policy: &LicensePolicy) -> bool {
        self.claims
            .as_ref()
            .is_some_and(|claims| policy.agentic_skus.contains(&claims.sku))
    }
}

/// Configurable SKU boundary. Product code uses `Default`; tests and future B2
/// fulfillment can supply a different paid-SKU set without changing parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicensePolicy {
    agentic_skus: BTreeSet<String>,
}

impl LicensePolicy {
    pub fn new(agentic_skus: impl IntoIterator<Item = String>) -> Self {
        Self {
            agentic_skus: agentic_skus.into_iter().collect(),
        }
    }
}

impl Default for LicensePolicy {
    fn default() -> Self {
        Self::new([DEFAULT_AGENTIC_SKU.to_string()])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenseError {
    InvalidFormat,
    UnsupportedPrefix,
    InvalidPayloadEncoding,
    InvalidSignatureEncoding,
    InvalidSignature,
    InvalidSchema(String),
}

impl fmt::Display for LicenseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => write!(f, "license key must have three dot-separated parts"),
            Self::UnsupportedPrefix => write!(f, "unsupported license key prefix"),
            Self::InvalidPayloadEncoding => write!(f, "license payload is not valid base64url"),
            Self::InvalidSignatureEncoding => {
                write!(f, "license signature is not a valid Ed25519 signature")
            }
            Self::InvalidSignature => write!(f, "license signature verification failed"),
            Self::InvalidSchema(reason) => write!(f, "invalid license payload: {reason}"),
        }
    }
}

impl std::error::Error for LicenseError {}

pub fn validate_license_key(key: &str) -> Result<LicenseClaims, LicenseError> {
    let mut parts = key.trim().split('.');
    let prefix = parts.next().ok_or(LicenseError::InvalidFormat)?;
    let payload_part = parts.next().ok_or(LicenseError::InvalidFormat)?;
    let signature_part = parts.next().ok_or(LicenseError::InvalidFormat)?;
    if parts.next().is_some() {
        return Err(LicenseError::InvalidFormat);
    }
    if prefix != LICENSE_PREFIX {
        return Err(LicenseError::UnsupportedPrefix);
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_part)
        .map_err(|_| LicenseError::InvalidPayloadEncoding)?;
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_part)
        .map_err(|_| LicenseError::InvalidSignatureEncoding)?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| LicenseError::InvalidSignatureEncoding)?;
    let verifying_key = VerifyingKey::from_bytes(&LICENSE_PUBLIC_KEY_BYTES)
        .map_err(|_| LicenseError::InvalidSignatureEncoding)?;
    verifying_key
        .verify_strict(&payload, &signature)
        .map_err(|_| LicenseError::InvalidSignature)?;

    let claims: LicenseClaims = serde_json::from_slice(&payload)
        .map_err(|error| LicenseError::InvalidSchema(error.to_string()))?;
    validate_claims(&claims)?;
    Ok(claims)
}

pub fn evaluate_license(
    key: Option<&str>,
    now: DateTime<Utc>,
    last_online_validation: Option<DateTime<Utc>>,
    validated_online_now: bool,
) -> Result<LicenseStatus, LicenseError> {
    let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(LicenseStatus::unlicensed());
    };
    let claims = validate_license_key(key)?;
    let state = if claims.updates_until < now.date_naive() {
        LicenseState::ExpiredUpdates
    } else if validated_online_now || last_online_validation.is_none() {
        LicenseState::Active
    } else {
        let elapsed_days = now
            .signed_duration_since(last_online_validation.expect("checked above"))
            .num_days();
        if elapsed_days <= 0 {
            LicenseState::Active
        } else if elapsed_days < OFFLINE_GRACE_DAYS {
            LicenseState::GraceOffline {
                days_left: u8::try_from(OFFLINE_GRACE_DAYS - elapsed_days)
                    .expect("offline grace is bounded to 30 days"),
            }
        } else {
            LicenseState::ExpiredUpdates
        }
    };
    Ok(LicenseStatus {
        state,
        claims: Some(claims),
    })
}

fn validate_claims(claims: &LicenseClaims) -> Result<(), LicenseError> {
    if claims.v != 1 {
        return Err(LicenseError::InvalidSchema("v must equal 1".into()));
    }
    if claims.sku.trim().is_empty() {
        return Err(LicenseError::InvalidSchema("sku must not be empty".into()));
    }
    if claims.email_hash.len() != 64
        || !claims
            .email_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LicenseError::InvalidSchema(
            "email_hash must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    if claims.seat_limit == 0 {
        return Err(LicenseError::InvalidSchema(
            "seat_limit must be greater than zero".into(),
        ));
    }
    if claims.updates_until < claims.issued {
        return Err(LicenseError::InvalidSchema(
            "updates_until must not precede issued".into(),
        ));
    }
    Ok(())
}

#[cfg(any(test, feature = "licensing-dev"))]
pub mod test_util {
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};

    use super::{LICENSE_PREFIX, LicenseClaims};

    /// RFC 8032 development key. Never use this helper for production issuance.
    const DEV_SIGNING_KEY_BYTES: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];

    pub fn sign_dev_license(claims: &LicenseClaims) -> String {
        let payload = serde_json::to_vec(claims).expect("development claims must serialize");
        let signature = SigningKey::from_bytes(&DEV_SIGNING_KEY_BYTES).sign(&payload);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let signature =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
        format!("{LICENSE_PREFIX}.{payload}.{signature}")
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn claims() -> LicenseClaims {
        LicenseClaims {
            v: 1,
            sku: DEFAULT_AGENTIC_SKU.into(),
            email_hash: "a".repeat(64),
            issued: NaiveDate::from_ymd_opt(2026, 8, 4).unwrap(),
            updates_until: NaiveDate::from_ymd_opt(2027, 8, 4).unwrap(),
            seat_limit: 3,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 4, 12, 0, 0).unwrap()
    }

    #[test]
    fn licensing_valid_key_is_active_and_entitles_agentic() {
        let key = test_util::sign_dev_license(&claims());
        let status = evaluate_license(Some(&key), now(), None, false).unwrap();
        assert_eq!(status.state, LicenseState::Active);
        assert!(status.allows_agentic(&LicensePolicy::default()));
    }

    #[test]
    fn licensing_rejects_bad_signature_schema_and_prefix() {
        let valid = test_util::sign_dev_license(&claims());
        let mut bad_signature = valid.clone().into_bytes();
        let last = bad_signature.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        assert!(matches!(
            validate_license_key(std::str::from_utf8(&bad_signature).unwrap()),
            Err(LicenseError::InvalidSignature)
        ));
        assert!(matches!(
            validate_license_key(&valid.replacen("CSK1", "CSK2", 1)),
            Err(LicenseError::UnsupportedPrefix)
        ));

        let mut invalid_claims = claims();
        invalid_claims.v = 2;
        let invalid_schema = test_util::sign_dev_license(&invalid_claims);
        assert!(matches!(
            validate_license_key(&invalid_schema),
            Err(LicenseError::InvalidSchema(_))
        ));
    }

    #[test]
    fn licensing_grace_counts_from_last_online_validation() {
        let key = test_util::sign_dev_license(&claims());
        let status =
            evaluate_license(Some(&key), now(), Some(now() - Duration::days(12)), false).unwrap();
        assert_eq!(status.state, LicenseState::GraceOffline { days_left: 18 });
    }

    #[test]
    fn licensing_thirty_day_boundary_flips_state() {
        let key = test_util::sign_dev_license(&claims());
        let at_29 =
            evaluate_license(Some(&key), now(), Some(now() - Duration::days(29)), false).unwrap();
        let at_30 =
            evaluate_license(Some(&key), now(), Some(now() - Duration::days(30)), false).unwrap();
        assert_eq!(at_29.state, LicenseState::GraceOffline { days_left: 1 });
        assert_eq!(at_30.state, LicenseState::ExpiredUpdates);
    }

    #[test]
    fn licensing_paid_sku_boundary_is_configurable() {
        let key = test_util::sign_dev_license(&claims());
        let status = evaluate_license(Some(&key), now(), None, false).unwrap();
        let other_policy = LicensePolicy::new(["future-pro".to_string()]);
        assert!(!status.allows_agentic(&other_policy));
    }
}
