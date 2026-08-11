//! Product truth for the Layer 1 ASR mode and audio-egress consent (C2).
//!
//! One brain, three modes: `cloud | local_power | apple_only`. The resolver in
//! this module is the only place the persisted mode string, the consent record,
//! and the legacy `use_local_stt` choice combine into a runtime decision — the
//! settings UI, the session factory, and future onboarding all consume
//! [`ResolvedAsrMode`] instead of re-deriving policy from raw fields.
//!
//! ## Doctrine encoded here
//!
//! - **Fresh install is Apple-only.** No persisted choice and no legacy signal
//!   resolves to [`AsrProductMode::AppleOnly`] — never a hidden local model
//!   load, never cloud.
//! - **Cloud requires explicit audio-egress consent.** A `cloud` mode value
//!   without a granted consent record resolves to Apple-only. Missing,
//!   unknown, or denied consent are all the same answer: no egress.
//! - **Upgrades preserve the prior local/cloud choice.** An installed user who
//!   explicitly persisted `use_local_stt` keeps the corresponding mode; a
//!   prior cloud choice carries its own consent evidence
//!   ([`ConsentSource::LegacyCloudChoice`]) because that user already
//!   configured and used an off-device transcription path on purpose.
//! - **No consent fallback may select local weights.** Every refusal lands on
//!   Apple-only; `local_power` is reachable only as an explicit choice.
//! - **The gateway mint config carries no vendor keys.** [`GatewaySessionMint`]
//!   is an endpoint, not a credential: URLs with user-info or query material
//!   are refused at construction, and there is no field a vendor key could
//!   occupy. Short-lived session bearers are minted by the Libraxis gateway
//!   outside the desktop and consumed by `asr_session::cloud`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Wire value for a granted audio-egress consent record.
pub const CONSENT_WIRE_GRANTED: &str = "granted";
/// Wire value for an explicitly denied audio-egress consent record.
pub const CONSENT_WIRE_DENIED: &str = "denied";

/// First-class Layer 1 product mode chosen by the user.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AsrProductMode {
    /// Normalized live cloud session behind the Libraxis gateway contract.
    Cloud,
    /// Power-user local helper with on-demand weights (killable process, L0).
    LocalPower,
    /// Apple canvas + lexicon only. The safe floor every failure resolves to.
    AppleOnly,
}

impl AsrProductMode {
    /// Stable persisted identifier; round-trips through [`FromStr`].
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::LocalPower => "local_power",
            Self::AppleOnly => "apple_only",
        }
    }

    /// Human-readable name for the settings UI. Presentation only.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Cloud => "Cloud (Libraxis gateway)",
            Self::LocalPower => "Local power (on-device weights)",
            Self::AppleOnly => "Apple only",
        }
    }

    /// Whether this mode sends captured audio off the machine.
    pub fn sends_audio_off_device(&self) -> bool {
        matches!(self, Self::Cloud)
    }
}

impl FromStr for AsrProductMode {
    type Err = String;

    /// Parse the persisted mode identifier. No aliases: an unknown value must
    /// fail loudly so the resolver can fall back to Apple-only instead of
    /// guessing a mode that moves audio or loads weights.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cloud" => Ok(Self::Cloud),
            "local_power" => Ok(Self::LocalPower),
            "apple_only" => Ok(Self::AppleOnly),
            other => Err(format!("Unknown AsrProductMode: {other}")),
        }
    }
}

/// Where a granted consent came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentSource {
    /// The user answered the explicit audio-egress consent question.
    ExplicitSettings,
    /// A pre-mode install had already chosen cloud transcription
    /// (`use_local_stt = false` persisted); the upgrade preserves that choice
    /// and records this derivation instead of re-asking.
    LegacyCloudChoice,
}

/// Typed audio-egress consent state.
///
/// Deliberately three-valued: "never asked" and "denied" both refuse egress,
/// but the settings UI needs to tell them apart (ask vs. respect the no).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEgressConsent {
    /// The user explicitly allowed sending captured audio off this machine.
    Granted(ConsentSource),
    /// The user explicitly refused. Only a new explicit grant changes this.
    Denied,
    /// No answer recorded. Resolves exactly like a denial: no egress.
    Unanswered,
}

impl AudioEgressConsent {
    /// Parse the persisted wire value. Anything other than the two canonical
    /// tokens (including tampered or truncated values) reads as
    /// [`Self::Unanswered`] — fail closed, never fail open.
    pub fn from_wire(wire: Option<&str>) -> Self {
        match wire.map(|value| value.trim().to_ascii_lowercase()) {
            Some(value) if value == CONSENT_WIRE_GRANTED => {
                Self::Granted(ConsentSource::ExplicitSettings)
            }
            Some(value) if value == CONSENT_WIRE_DENIED => Self::Denied,
            _ => Self::Unanswered,
        }
    }

    /// Whether audio may leave the machine under this consent state.
    pub fn permits_egress(&self) -> bool {
        matches!(self, Self::Granted(_))
    }
}

/// Why the resolver picked the mode it picked. Diagnostics and UI copy only —
/// never a second policy axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeDerivation {
    /// The persisted `asr_mode` value was honored as written.
    ExplicitChoice,
    /// No mode persisted; the prior `use_local_stt = true` choice carried over.
    LegacyLocalChoice,
    /// No mode persisted; the prior `use_local_stt = false` cloud choice
    /// carried over together with its derived consent.
    LegacyCloudChoice,
    /// Fresh install: no mode, no legacy signal. The safe floor.
    FreshDefault,
    /// `cloud` was persisted but no consent record exists. Egress refused.
    ConsentMissingFallback,
    /// `cloud` was persisted but consent is explicitly denied. Egress refused.
    ConsentDeniedFallback,
    /// The persisted mode value did not parse. Refuse to guess.
    UnknownModeFallback,
}

/// The single resolved answer the rest of the product consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedAsrMode {
    /// Effective product mode after consent and legacy derivation.
    pub mode: AsrProductMode,
    /// Consent state the mode was resolved under.
    pub consent: AudioEgressConsent,
    /// How the resolver arrived here.
    pub derivation: ModeDerivation,
}

/// Resolve the effective Layer 1 product mode from the persisted mode string,
/// the persisted consent wire value, and the legacy local/cloud switch.
///
/// Pure on purpose: no I/O, no env, no clock. Every input combination has an
/// asserted answer in the test matrix below, and every refusal lands on
/// [`AsrProductMode::AppleOnly`] — never on [`AsrProductMode::LocalPower`].
pub fn resolve_asr_product_mode(
    explicit_mode: Option<&str>,
    consent_wire: Option<&str>,
    legacy_use_local_stt: Option<bool>,
) -> ResolvedAsrMode {
    let consent = AudioEgressConsent::from_wire(consent_wire);

    let explicit = explicit_mode
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(raw) = explicit {
        return match raw.parse::<AsrProductMode>() {
            Ok(AsrProductMode::Cloud) => match consent {
                AudioEgressConsent::Granted(_) => ResolvedAsrMode {
                    mode: AsrProductMode::Cloud,
                    consent,
                    derivation: ModeDerivation::ExplicitChoice,
                },
                AudioEgressConsent::Denied => ResolvedAsrMode {
                    mode: AsrProductMode::AppleOnly,
                    consent,
                    derivation: ModeDerivation::ConsentDeniedFallback,
                },
                AudioEgressConsent::Unanswered => ResolvedAsrMode {
                    mode: AsrProductMode::AppleOnly,
                    consent,
                    derivation: ModeDerivation::ConsentMissingFallback,
                },
            },
            Ok(mode) => ResolvedAsrMode {
                mode,
                consent,
                derivation: ModeDerivation::ExplicitChoice,
            },
            Err(_) => ResolvedAsrMode {
                mode: AsrProductMode::AppleOnly,
                consent,
                derivation: ModeDerivation::UnknownModeFallback,
            },
        };
    }

    match legacy_use_local_stt {
        Some(true) => ResolvedAsrMode {
            mode: AsrProductMode::LocalPower,
            consent,
            derivation: ModeDerivation::LegacyLocalChoice,
        },
        Some(false) => {
            // A persisted cloud choice predating the mode field. An explicit
            // denial recorded since then wins over the derived grant.
            if consent == AudioEgressConsent::Denied {
                ResolvedAsrMode {
                    mode: AsrProductMode::AppleOnly,
                    consent,
                    derivation: ModeDerivation::ConsentDeniedFallback,
                }
            } else {
                let consent = match consent {
                    AudioEgressConsent::Granted(_) => consent,
                    _ => AudioEgressConsent::Granted(ConsentSource::LegacyCloudChoice),
                };
                ResolvedAsrMode {
                    mode: AsrProductMode::Cloud,
                    consent,
                    derivation: ModeDerivation::LegacyCloudChoice,
                }
            }
        }
        None => ResolvedAsrMode {
            mode: AsrProductMode::AppleOnly,
            consent,
            derivation: ModeDerivation::FreshDefault,
        },
    }
}

/// Why a gateway session-mint endpoint was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayMintError {
    /// Not a parseable absolute URL, or no host.
    InvalidUrl,
    /// Remote plaintext. `https` is required off loopback.
    InsecureScheme,
    /// URL user-info (`user:pass@`) — credentials never live in this config.
    EmbeddedCredentials,
    /// Query/fragment material — signed parameters and keys belong to the
    /// minted session response, never to the persisted endpoint.
    QueryNotAllowed,
}

impl fmt::Display for GatewayMintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidUrl => "gateway mint URL is not a valid absolute URL",
            Self::InsecureScheme => "gateway mint URL must be https (http is loopback-only)",
            Self::EmbeddedCredentials => "gateway mint URL must not embed credentials",
            Self::QueryNotAllowed => "gateway mint URL must not carry query or fragment data",
        };
        f.write_str(text)
    }
}

/// Validated Libraxis gateway session-mint endpoint.
///
/// The desktop POSTs here to obtain a short-lived session (endpoint + bearer)
/// and hands the result to `asr_session::cloud::GatewayConnection`. By
/// construction this type holds an endpoint and nothing else: there is no
/// vendor key field, and URLs that try to smuggle credential material are
/// refused. Provider choice stays behind the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewaySessionMint {
    url: String,
}

impl GatewaySessionMint {
    /// Validate a session-mint endpoint URL.
    pub fn new(raw: &str) -> Result<Self, GatewayMintError> {
        let raw = raw.trim();
        let parsed = reqwest::Url::parse(raw).map_err(|_| GatewayMintError::InvalidUrl)?;
        let host = parsed
            .host_str()
            .map(|value| value.trim_matches(['[', ']']))
            .ok_or(GatewayMintError::InvalidUrl)?;
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
        match parsed.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => return Err(GatewayMintError::InsecureScheme),
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(GatewayMintError::EmbeddedCredentials);
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(GatewayMintError::QueryNotAllowed);
        }
        Ok(Self {
            url: raw.to_string(),
        })
    }

    /// The validated endpoint URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Resolution matrix, wire parsing, and mint-endpoint validation contracts.
#[cfg(test)]
mod tests {
    use super::*;

    /// Mode identifiers round-trip and unknown values fail loudly.
    #[test]
    fn asr_mode_wire_round_trip() {
        for mode in [
            AsrProductMode::Cloud,
            AsrProductMode::LocalPower,
            AsrProductMode::AppleOnly,
        ] {
            assert_eq!(mode.as_str().parse::<AsrProductMode>(), Ok(mode));
        }
        assert!("whisper".parse::<AsrProductMode>().is_err());
        assert!("".parse::<AsrProductMode>().is_err());
    }

    /// Only Cloud classifies as sending audio off the device.
    #[test]
    fn only_cloud_sends_audio_off_device() {
        assert!(AsrProductMode::Cloud.sends_audio_off_device());
        assert!(!AsrProductMode::LocalPower.sends_audio_off_device());
        assert!(!AsrProductMode::AppleOnly.sends_audio_off_device());
    }

    /// Consent wire parsing fails closed on anything non-canonical.
    #[test]
    fn consent_wire_fails_closed() {
        assert_eq!(
            AudioEgressConsent::from_wire(Some("granted")),
            AudioEgressConsent::Granted(ConsentSource::ExplicitSettings)
        );
        assert_eq!(
            AudioEgressConsent::from_wire(Some("denied")),
            AudioEgressConsent::Denied
        );
        for garbage in [None, Some(""), Some("yes"), Some("1"), Some("GRANTED!")] {
            assert_eq!(
                AudioEgressConsent::from_wire(garbage),
                AudioEgressConsent::Unanswered,
                "non-canonical wire {garbage:?} must read as Unanswered"
            );
        }
        // Canonical values are case/whitespace tolerant but nothing more.
        assert!(AudioEgressConsent::from_wire(Some(" Granted ")).permits_egress());
    }

    /// Fresh install: no mode, no legacy signal, no consent → Apple-only.
    #[test]
    fn fresh_install_resolves_apple_only() {
        let resolved = resolve_asr_product_mode(None, None, None);
        assert_eq!(resolved.mode, AsrProductMode::AppleOnly);
        assert_eq!(resolved.derivation, ModeDerivation::FreshDefault);
        assert!(!resolved.consent.permits_egress());
    }

    /// Upgrade preservation: a persisted local choice stays local.
    #[test]
    fn upgrade_preserves_legacy_local_choice() {
        let resolved = resolve_asr_product_mode(None, None, Some(true));
        assert_eq!(resolved.mode, AsrProductMode::LocalPower);
        assert_eq!(resolved.derivation, ModeDerivation::LegacyLocalChoice);
    }

    /// Upgrade preservation: a persisted cloud choice stays cloud, carrying a
    /// derived consent instead of silently re-asking or silently refusing.
    #[test]
    fn upgrade_preserves_legacy_cloud_choice_with_derived_consent() {
        let resolved = resolve_asr_product_mode(None, None, Some(false));
        assert_eq!(resolved.mode, AsrProductMode::Cloud);
        assert_eq!(resolved.derivation, ModeDerivation::LegacyCloudChoice);
        assert_eq!(
            resolved.consent,
            AudioEgressConsent::Granted(ConsentSource::LegacyCloudChoice)
        );
    }

    /// An explicit denial recorded after the upgrade beats the legacy grant.
    #[test]
    fn explicit_denial_beats_legacy_cloud_choice() {
        let resolved = resolve_asr_product_mode(None, Some("denied"), Some(false));
        assert_eq!(resolved.mode, AsrProductMode::AppleOnly);
        assert_eq!(resolved.derivation, ModeDerivation::ConsentDeniedFallback);
    }

    /// Explicit cloud with granted consent is honored.
    #[test]
    fn explicit_cloud_with_consent_is_cloud() {
        let resolved = resolve_asr_product_mode(Some("cloud"), Some("granted"), None);
        assert_eq!(resolved.mode, AsrProductMode::Cloud);
        assert_eq!(resolved.derivation, ModeDerivation::ExplicitChoice);
    }

    /// Explicit cloud without consent resolves to Apple-only — and never to
    /// local weights, even when a legacy local signal is also present.
    #[test]
    fn cloud_without_consent_resolves_apple_only_never_local() {
        for (consent, derivation) in [
            (None, ModeDerivation::ConsentMissingFallback),
            (Some("denied"), ModeDerivation::ConsentDeniedFallback),
            (Some("tampered"), ModeDerivation::ConsentMissingFallback),
        ] {
            for legacy in [None, Some(true), Some(false)] {
                let resolved = resolve_asr_product_mode(Some("cloud"), consent, legacy);
                assert_eq!(
                    resolved.mode,
                    AsrProductMode::AppleOnly,
                    "cloud with consent={consent:?} legacy={legacy:?} must refuse egress"
                );
                assert_eq!(resolved.derivation, derivation);
            }
        }
    }

    /// Explicit non-cloud modes need no consent record.
    #[test]
    fn explicit_local_and_apple_need_no_consent() {
        let local = resolve_asr_product_mode(Some("local_power"), None, None);
        assert_eq!(local.mode, AsrProductMode::LocalPower);
        assert_eq!(local.derivation, ModeDerivation::ExplicitChoice);

        let apple = resolve_asr_product_mode(Some("apple_only"), Some("granted"), Some(false));
        assert_eq!(apple.mode, AsrProductMode::AppleOnly);
        assert_eq!(apple.derivation, ModeDerivation::ExplicitChoice);
    }

    /// An unparseable persisted mode refuses to guess: Apple-only, not legacy
    /// derivation and not local weights.
    #[test]
    fn unknown_mode_value_resolves_apple_only() {
        let resolved = resolve_asr_product_mode(Some("turbo_cloud"), Some("granted"), Some(true));
        assert_eq!(resolved.mode, AsrProductMode::AppleOnly);
        assert_eq!(resolved.derivation, ModeDerivation::UnknownModeFallback);
    }

    /// Mint endpoint accepts clean https (and loopback http for dev).
    #[test]
    fn gateway_mint_accepts_clean_endpoints() {
        for url in [
            "https://gateway.libraxis.cloud/v1/asr/sessions",
            "https://gateway.libraxis.cloud/mint",
            "http://127.0.0.1:8089/mint",
            "http://localhost:8089/mint",
        ] {
            let mint = GatewaySessionMint::new(url).expect("clean endpoint accepted");
            assert_eq!(mint.url(), url);
        }
    }

    /// Mint endpoint refuses anything that could smuggle a credential: remote
    /// plaintext, user-info, query strings, fragments, relative junk.
    #[test]
    fn gateway_mint_refuses_credential_material() {
        let cases = [
            (
                "http://gateway.libraxis.cloud/mint",
                GatewayMintError::InsecureScheme,
            ),
            (
                "ftp://gateway.libraxis.cloud/mint",
                GatewayMintError::InsecureScheme,
            ),
            (
                "https://user:secret@gateway.libraxis.cloud/mint",
                GatewayMintError::EmbeddedCredentials,
            ),
            (
                "https://gateway.libraxis.cloud/mint?api_key=sk-123",
                GatewayMintError::QueryNotAllowed,
            ),
            (
                "https://gateway.libraxis.cloud/mint#token",
                GatewayMintError::QueryNotAllowed,
            ),
            ("not a url", GatewayMintError::InvalidUrl),
            ("", GatewayMintError::InvalidUrl),
        ];
        for (url, expected) in cases {
            assert_eq!(
                GatewaySessionMint::new(url),
                Err(expected),
                "endpoint {url:?} must be refused"
            );
        }
    }
}
