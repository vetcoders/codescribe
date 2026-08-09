//! The build-time contract between a binary and the key it verifies licenses
//! with. Kept in its own leaf module so `build.rs` and the runtime verifier
//! read the same constants and cannot drift apart.

/// Build-time variable carrying the production Ed25519 public key (hex). The
/// operator sets it; a release build without it stays fail-closed.
pub const LICENSE_PUBLIC_KEY_ENV: &str = "CODESCRIBE_LICENSE_PUBLIC_KEY_HEX";
/// Development verifier key, checked in on purpose. Its private half is the
/// RFC 8032 test vector, so licenses under it are forgeable by anyone —
/// acceptable for local builds, never for a distributed artifact.
pub const DEV_LICENSE_PUBLIC_KEY_HEX: &str =
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
/// SHA-256 of [`DEV_LICENSE_PUBLIC_KEY_HEX`], so a build can name which key it
/// verifies against without printing the key itself.
pub const DEV_LICENSE_PUBLIC_KEY_FINGERPRINT: &str =
    "21fe31dfa154a261626bf854046fd2271b7bed4b6abe45aa58877ef47f9721b9";
