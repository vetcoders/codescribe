//! The audio-egress consent gate in front of Layer 1 session construction.
//!
//! [`super::provider::RefinerMode::sends_audio_off_device`] is the classifier;
//! this module is the owner that asks it. A cloud session cannot be authorized
//! without an explicit granted consent record, and every refusal degrades to
//! [`RefinerMode::Off`] (Apple canvas + lexicon) — never to a local model.
//!
//! The gate produces a [`CloudEgressAuthorization`] witness. The type has no
//! public constructor, so recorder/transport wiring that opens a real
//! [`super::cloud::LiveCloudAsrSession`] against a minted gateway session must
//! have passed through [`authorize_cloud_egress`] to hold one — consent
//! enforcement is structural, not a convention callers remember to follow.

use crate::config::cloud_asr::{AsrProductMode, AudioEgressConsent, ResolvedAsrMode};

use super::provider::RefinerMode;

/// Typed refusal from the Layer 1 session factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSessionError {
    /// Audio egress was requested without an explicit granted consent record.
    ConsentRequired,
}

/// Proof that explicit audio-egress consent backs a cloud session.
///
/// Constructible only through [`authorize_cloud_egress`]. Deliberately not
/// `Clone`/`Copy`: one authorization, one session.
#[derive(Debug, PartialEq, Eq)]
pub struct CloudEgressAuthorization {
    _witness: (),
}

/// Authorize opening a cloud Layer 1 session under the given consent state.
///
/// This is the production factory gate the fleet RED precommitted: session
/// construction without explicit audio-egress consent is rejected with a typed
/// [`CloudSessionError::ConsentRequired`], never a panic and never a silent
/// downgrade to a different provider.
pub fn authorize_cloud_egress(
    consent: &AudioEgressConsent,
) -> Result<CloudEgressAuthorization, CloudSessionError> {
    if consent.permits_egress() {
        Ok(CloudEgressAuthorization { _witness: () })
    } else {
        Err(CloudSessionError::ConsentRequired)
    }
}

/// Map the resolved product mode onto the Layer 1 refiner axis.
///
/// The consent gate is re-asked here even though the resolver already enforced
/// it — defense in depth for a hand-built [`ResolvedAsrMode`]. Every refusal
/// lands on [`RefinerMode::Off`]: a missing consent can suppress cloud, but it
/// can never promote local weights nobody opted into.
pub fn refiner_for(resolved: &ResolvedAsrMode) -> RefinerMode {
    match resolved.mode {
        AsrProductMode::Cloud => match authorize_cloud_egress(&resolved.consent) {
            Ok(_) => RefinerMode::CloudSession,
            Err(CloudSessionError::ConsentRequired) => RefinerMode::Off,
        },
        AsrProductMode::LocalPower => RefinerMode::LocalHelper,
        AsrProductMode::AppleOnly => RefinerMode::Off,
    }
}

/// Consent-gate unit contracts; the fleet-level witness lives in
/// `crate::stt::fleet_red_contracts`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::cloud_asr::{ConsentSource, resolve_asr_product_mode};

    /// Unanswered and denied consent both refuse authorization with the typed
    /// error; a granted record (either source) passes.
    #[test]
    fn consent_gate_refuses_without_explicit_grant() {
        for refused in [AudioEgressConsent::Unanswered, AudioEgressConsent::Denied] {
            assert_eq!(
                authorize_cloud_egress(&refused),
                Err(CloudSessionError::ConsentRequired)
            );
        }
        for source in [
            ConsentSource::ExplicitSettings,
            ConsentSource::LegacyCloudChoice,
        ] {
            assert!(authorize_cloud_egress(&AudioEgressConsent::Granted(source)).is_ok());
        }
    }

    /// Mode-to-refiner mapping: consent-backed cloud arms the cloud session,
    /// explicit local power arms the helper, and every refusal is Off — the
    /// degraded shape can never be a local model load.
    #[test]
    fn refiner_mapping_degrades_to_off_never_local() {
        let cloud = resolve_asr_product_mode(Some("cloud"), Some("granted"), None);
        assert_eq!(refiner_for(&cloud), RefinerMode::CloudSession);

        let local = resolve_asr_product_mode(Some("local_power"), None, None);
        assert_eq!(refiner_for(&local), RefinerMode::LocalHelper);

        for resolved in [
            resolve_asr_product_mode(Some("cloud"), None, None),
            resolve_asr_product_mode(Some("cloud"), Some("denied"), Some(true)),
            resolve_asr_product_mode(None, None, None),
            resolve_asr_product_mode(Some("apple_only"), Some("granted"), None),
        ] {
            assert_eq!(
                refiner_for(&resolved),
                RefinerMode::Off,
                "refusal for {resolved:?} must degrade to Off, never LocalHelper"
            );
        }
    }

    /// The privacy bound on telemetry holds at the type level: session
    /// counters are `Copy` (no heap text can hide in them), and the typed
    /// error vocabulary carries no payload a transcript could ride on.
    #[test]
    fn telemetry_and_errors_stay_content_free() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<super::super::cloud::CloudSessionTelemetry>();
        assert_copy::<super::super::events::AsrErrorKind>();
        assert_copy::<CloudSessionError>();
    }
}
