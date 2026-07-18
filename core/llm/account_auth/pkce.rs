// Portions derived from openai/codex (Apache-2.0).

use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

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

pub fn challenge_for_verifier(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for_verifier(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

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
