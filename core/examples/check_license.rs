use codescribe_core::licensing::{LICENSE_PUBLIC_KEY_FINGERPRINT, validate_license_key};

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
