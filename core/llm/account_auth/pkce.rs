//! PKCE (RFC 7636) code generation for the subscription-account OAuth flow.
//!
//! The desktop app is a public client — it cannot keep a secret — so the
//! authorization code is bound to a one-time verifier instead. Only the
//! `S256` challenge travels with the authorization request; the verifier stays
//! in memory until the token exchange proves the same client is redeeming it.

// Portions derived from openai/codex (Apache-2.0).

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// One PKCE pair for a single authorization attempt. Never reuse across flows:
/// the verifier is the only proof the redeemer is the requester.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceCodes {
    /// High-entropy secret held locally and sent only at token exchange.
    pub code_verifier: String,
    /// `S256` transform of the verifier, safe to put in the authorization URL.
    pub code_challenge: String,
}

/// Generate a fresh verifier/challenge pair from 64 bytes of OS entropy.
pub fn generate_pkce() -> PkceCodes {
    let mut bytes = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let code_challenge = challenge_for_verifier(&code_verifier);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

/// Derive the `S256` challenge for a verifier: base64url-no-pad of its
/// SHA-256 digest, exactly as RFC 7636 specifies.
pub fn challenge_for_verifier(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// RFC 7636 vector + generated-verifier length/challenge self-consistency checks.
#[cfg(test)]
mod tests {
    use super::*;

    /// Pins challenge_for_verifier to the RFC 7636 Appendix B S256 test vector.
    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for_verifier(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// Fresh verifiers stay in the 43..128 char band and match their stored challenge.
    #[test]
    fn generated_verifier_uses_valid_pkce_length() {
        let codes = generate_pkce();
        assert!((43..=128).contains(&codes.code_verifier.len()));
        assert_eq!(
            challenge_for_verifier(&codes.code_verifier),
            codes.code_challenge
        );
    }
}
