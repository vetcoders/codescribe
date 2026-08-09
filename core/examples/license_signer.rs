//! Offline CSK1 production license tool. Runs on the operator machine only —
//! the private seed never enters this repository, the app bundle, or CI.
//!
//! Key generation (run once, store the seed in the operator credential store):
//! ```bash
//! cargo run -p codescribe-core --example license_signer -- keygen
//! ```
//!
//! Issuance (seed supplied at runtime, never as an argument):
//! ```bash
//! CODESCRIBE_LICENSE_SIGNER_SEED_HEX=<64-hex> \
//!   cargo run -p codescribe-core --example license_signer -- sign monika@vetcoders.io
//! ```
//!
//! The dev seed (RFC 8032 test vector) is publicly known; this tool refuses to
//! sign with it. Development issuance stays in `generate_dev_license`.

use std::io::Write as _;
use std::process::ExitCode;

use base64::Engine as _;
use chrono::{Months, Utc};
use codescribe_core::licensing::{DEFAULT_AGENTIC_SKU, LICENSE_PREFIX, LicenseClaims};
use ed25519_dalek::{Signer, SigningKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// RFC 8032 test-vector seed — the checked-in development signer. Publicly
/// known, therefore forgeable; production issuance must never use it.
const DEV_SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

/// Route to `keygen` or `sign`; anything else prints usage and exits 2.
fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("keygen") => keygen(),
        Some("sign") => sign(args.collect()),
        _ => {
            eprintln!("usage: license_signer keygen");
            eprintln!("       license_signer sign <email> [sku] [updates-months] [seat-limit]");
            ExitCode::from(2)
        }
    }
}

/// Mint a fresh Ed25519 signing key.
///
/// The public half and its fingerprint go to stdout, ready to paste into the
/// repository variable. The private seed goes to stderr and is zeroed
/// immediately after — split across the two streams so a redirected stdout
/// cannot capture the secret alongside the shareable half.
fn keygen() -> ExitCode {
    let mut seed = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let public_hex = hex(signing.verifying_key().as_bytes());
    let fingerprint = hex(&Sha256::digest(public_hex.as_bytes()));

    println!("public key (repository variable, safe to share):");
    println!(
        "  gh variable set CODESCRIBE_LICENSE_PUBLIC_KEY_HEX --repo vetcoders/codescribe --body {public_hex}"
    );
    println!("public key fingerprint: {fingerprint}");
    println!();
    println!("PRIVATE SEED — shown once, store it in the operator credential store,");
    println!("never in a repository, chat, issue, or CI log:");
    let _ = writeln!(std::io::stderr(), "{}", hex(&seed));
    seed.fill(0);
    ExitCode::SUCCESS
}

/// Issue a signed CSK1 license for `<email> [sku] [updates-months] [seat-limit]`.
///
/// The seed arrives through the environment, never as an argument, so it stays
/// out of the process list and shell history. Signing with the publicly known
/// development seed is refused outright — a license anyone can forge must not
/// be issuable by the production tool.
fn sign(args: Vec<String>) -> ExitCode {
    let Some(email) = args.first() else {
        eprintln!("usage: license_signer sign <email> [sku] [updates-months] [seat-limit]");
        return ExitCode::from(2);
    };
    let sku = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| DEFAULT_AGENTIC_SKU.to_string());
    let updates_months: u32 = match args.get(2).map_or(Ok(12), |raw| raw.parse()) {
        Ok(months) => months,
        Err(_) => {
            eprintln!("updates-months must be a whole number of months");
            return ExitCode::from(2);
        }
    };
    let seat_limit: u16 = match args.get(3).map_or(Ok(3), |raw| raw.parse()) {
        Ok(seats) => seats,
        Err(_) => {
            eprintln!("seat-limit must fit in u16");
            return ExitCode::from(2);
        }
    };

    let Ok(seed_hex) = std::env::var("CODESCRIBE_LICENSE_SIGNER_SEED_HEX") else {
        eprintln!("CODESCRIBE_LICENSE_SIGNER_SEED_HEX is not set; refusing to guess a key");
        return ExitCode::FAILURE;
    };
    let seed_hex = seed_hex.trim().to_lowercase();
    if seed_hex == DEV_SEED_HEX {
        eprintln!(
            "refusing to sign with the development seed (RFC 8032 test vector — publicly known)"
        );
        eprintln!("development licenses come from the generate_dev_license example");
        return ExitCode::FAILURE;
    }
    let Some(seed) = decode_seed(&seed_hex) else {
        eprintln!("seed must be exactly 64 hexadecimal characters");
        return ExitCode::FAILURE;
    };

    let today = Utc::now().date_naive();
    let updates_until = today
        .checked_add_months(Months::new(updates_months))
        .unwrap_or(today);
    let claims = LicenseClaims {
        v: 1,
        sku,
        email_hash: hex(&Sha256::digest(email.trim().to_lowercase().as_bytes())),
        issued: today,
        updates_until,
        seat_limit,
    };

    let payload = serde_json::to_vec(&claims).expect("license claims serialize");
    let signature = SigningKey::from_bytes(&seed).sign(&payload);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    println!(
        "{LICENSE_PREFIX}.{}.{}",
        b64.encode(payload),
        b64.encode(signature.to_bytes())
    );
    eprintln!(
        "issued {} · updates until {} ({} seat(s)) — verify against a build carrying the matching public key",
        claims.issued, claims.updates_until, claims.seat_limit
    );
    ExitCode::SUCCESS
}

/// Parse exactly 64 hex characters into a 32-byte seed. Any other length, or
/// a non-hex digit, yields `None` rather than a truncated or padded key.
fn decode_seed(hex_str: &str) -> Option<[u8; 32]> {
    if hex_str.len() != 64 {
        return None;
    }
    let mut seed = [0_u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex_str[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(seed)
}

/// Lowercase hex encoding, used for keys, fingerprints and email hashes alike.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
