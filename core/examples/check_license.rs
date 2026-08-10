//! Verify a CSK1 license key against the public key this build carries.
//!
//! ```text
//! cargo run -p codescribe-core --example check_license -- <CSK1-key>
//! ```
//!
//! Both outcomes name the verifying fingerprint, because "rejected" is
//! ambiguous on its own: a valid key checked against the wrong build looks
//! identical to a forged one until you can see which key did the checking.

use codescribe_core::licensing::{LICENSE_PUBLIC_KEY_FINGERPRINT, validate_license_key};

/// Exit 0 when the key validates, 1 when it is rejected, 2 when none was given.
fn main() {
    let Some(key) = std::env::args().nth(1) else {
        eprintln!("usage: check_license <CSK1-key>");
        std::process::exit(2);
    };
    match validate_license_key(&key) {
        Ok(_) => println!("accepted by fingerprint {LICENSE_PUBLIC_KEY_FINGERPRINT}"),
        Err(error) => {
            eprintln!("rejected by fingerprint {LICENSE_PUBLIC_KEY_FINGERPRINT}: {error}");
            std::process::exit(1);
        }
    }
}
