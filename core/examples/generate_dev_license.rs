//! Issue a development license signed with the checked-in test-vector seed.
//!
//! ```text
//! cargo run -p codescribe-core --example generate_dev_license --features licensing-dev -- [email]
//! ```
//!
//! Deliberately separate from `license_signer`, which refuses that seed: keys
//! from here are forgeable by anyone holding the public RFC 8032 vector, so
//! they must never leave a developer machine. The feature gate is what keeps
//! the signing path out of ordinary builds.

/// Print a signed development license for the given email (or a local default).
///
/// Dates are fixed rather than derived from the clock, so the same invocation
/// yields the same claims and a diff of two runs shows only the email hash.
#[cfg(feature = "licensing-dev")]
fn main() {
    use chrono::NaiveDate;
    use codescribe_core::licensing::{DEFAULT_AGENTIC_SKU, LicenseClaims, test_util};
    use sha2::{Digest, Sha256};

    let email = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "dev@codescribe.local".to_string())
        .trim()
        .to_lowercase();
    let email_hash = format!("{:x}", Sha256::digest(email.as_bytes()));
    let claims = LicenseClaims {
        v: 1,
        sku: DEFAULT_AGENTIC_SKU.to_string(),
        email_hash,
        issued: NaiveDate::from_ymd_opt(2026, 8, 4).expect("valid development date"),
        updates_until: NaiveDate::from_ymd_opt(2027, 8, 4).expect("valid development date"),
        seat_limit: 3,
    };
    println!("{}", test_util::sign_dev_license(&claims));
}

/// Without the feature there is no signer to call, so say which flag is
/// missing and exit 2 rather than failing to build.
#[cfg(not(feature = "licensing-dev"))]
fn main() {
    eprintln!("rerun with --features licensing-dev");
    std::process::exit(2);
}
