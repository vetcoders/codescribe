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

#[cfg(not(feature = "licensing-dev"))]
fn main() {
    eprintln!("rerun with --features licensing-dev");
    std::process::exit(2);
}
