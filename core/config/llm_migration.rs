//! One-shot, lossless migration of the legacy LLM lane fields into the
//! provider registry shape (`00_ATLAS.md §B.1`, rule 5 corrected by `§B.3`).
//!
//! Legacy `settings.json` carried a per-lane `llm_endpoint` / `llm_model`
//! (plus a shared `speech.llm_endpoint` / `speech.llm_model` fallback and
//! `speech.assistive.provider`). The registry keeps vendor endpoints in code,
//! so a legacy endpoint becomes either a vendor reference (host matches a
//! pinned vendor) or a Custom provider row (`id = slug(host)`), and each lane
//! records `<lane>_provider` + `<lane>_model`. Keys move per lane into the
//! resolved provider's account; nothing is copied globally.

use tracing::info;

use super::keychain::KeyMove;
use super::settings::{RuntimeLlmLaneKind, UserSettings};
use crate::llm::provider::{
    ALL_PROVIDERS, CustomProvider, ProviderKind, ProviderRef, endpoint_host,
};

/// The legacy on-disk fields, read once from `SettingsV2` and never written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechV2Legacy {
    /// `speech.llm_endpoint` — shared fallback for both lanes.
    pub llm_endpoint: Option<String>,
    /// `speech.llm_model` — shared fallback for both lanes.
    pub llm_model: Option<String>,
    /// `speech.formatting.llm_endpoint`.
    pub formatting_endpoint: Option<String>,
    /// `speech.formatting.llm_model`.
    pub formatting_model: Option<String>,
    /// `speech.assistive.llm_endpoint`.
    pub assistive_endpoint: Option<String>,
    /// `speech.assistive.llm_model`.
    pub assistive_model: Option<String>,
    /// `speech.assistive.provider` (vendor spelling or alias).
    pub assistive_provider: Option<String>,
}

impl SpeechV2Legacy {
    /// Read the legacy fields out of the raw document, V2 (`speech.*`) or the
    /// flat V1 shape, without the typed schema ever carrying them.
    pub fn from_json(raw: &serde_json::Value) -> Self {
        let field = |v2: &str, v1: &str| -> Option<String> {
            raw.pointer(v2)
                .or_else(|| raw.pointer(v1))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        Self {
            llm_endpoint: field("/speech/llm_endpoint", "/llm_endpoint"),
            llm_model: field("/speech/llm_model", "/llm_model"),
            formatting_endpoint: field(
                "/speech/formatting/llm_endpoint",
                "/llm_formatting_endpoint",
            ),
            formatting_model: field("/speech/formatting/llm_model", "/llm_formatting_model"),
            assistive_endpoint: field("/speech/assistive/llm_endpoint", "/llm_assistive_endpoint"),
            assistive_model: field("/speech/assistive/llm_model", "/llm_assistive_model"),
            assistive_provider: field("/speech/assistive/provider", "/llm_assistive_provider"),
        }
    }

    /// True when a field that no longer exists in the schema is present — the
    /// trigger for running the migration. Per-lane models and the assistive
    /// provider are current-schema fields and never trigger on their own.
    pub fn needs_migration(&self) -> bool {
        [
            &self.llm_endpoint,
            &self.llm_model,
            &self.formatting_endpoint,
            &self.assistive_endpoint,
        ]
        .into_iter()
        .any(Option::is_some)
    }
}

/// Legacy Keychain account each lane read its key from.
const fn legacy_lane_account(lane: RuntimeLlmLaneKind) -> &'static str {
    match lane {
        RuntimeLlmLaneKind::Formatting => "LLM_FORMATTING_API_KEY",
        RuntimeLlmLaneKind::Assistive => "LLM_ASSISTIVE_API_KEY",
    }
}

/// The vendor whose pinned or historical host matches `endpoint`, if any.
fn vendor_for_endpoint(endpoint: &str) -> Option<ProviderKind> {
    let host = endpoint_host(endpoint);
    (!host.is_empty()).then(|| {
        ALL_PROVIDERS
            .into_iter()
            .find(|kind| kind.matches_host(host))
    })?
}

/// Resolve one legacy `(provider, endpoint)` pair to a provider reference,
/// adding a Custom row to `settings` when the host is not a vendor. Only the
/// same normalized endpoint and wire may share a row; a host alone is not identity.
fn resolve_legacy_endpoint(
    legacy_provider: ProviderKind,
    endpoint: Option<&str>,
    settings: &mut UserSettings,
) -> ProviderRef {
    let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) else {
        return ProviderRef::Vendor(legacy_provider);
    };
    if let Some(vendor) = vendor_for_endpoint(endpoint) {
        return ProviderRef::Vendor(vendor);
    }
    let host = endpoint_host(endpoint);
    let wire = legacy_provider.wire_family();
    match CustomProvider::new(host, wire, endpoint) {
        Ok(mut row) => {
            if let Some(existing) = settings
                .llm_custom_providers
                .iter()
                .find(|existing| existing.endpoint == row.endpoint && existing.wire == row.wire)
            {
                return ProviderRef::Custom(existing.id.clone());
            }
            let base_id = row.id.clone();
            let mut suffix = 2;
            while settings
                .llm_custom_providers
                .iter()
                .any(|existing| existing.id == row.id)
            {
                row.id = format!("{base_id}-{suffix}");
                suffix += 1;
            }
            let id = row.id.clone();
            settings.llm_custom_providers.push(row);
            ProviderRef::Custom(id)
        }
        Err(error) => {
            info!(
                "legacy endpoint {endpoint:?} is not a usable custom provider ({error}); lane keeps {legacy_provider}"
            );
            ProviderRef::Vendor(legacy_provider)
        }
    }
}

/// Migrate the legacy lane fields into `settings` and return the key moves the
/// loader must apply to the Keychain bundle.
///
/// Pure over its inputs: no I/O, no env. Idempotent by construction — the
/// legacy fields are gone after the first save, so a second pass sees
/// `needs_migration() == false` and the caller never invokes this again.
pub fn migrate_legacy_llm_lanes(
    legacy: &SpeechV2Legacy,
    settings: &mut UserSettings,
) -> Vec<KeyMove> {
    let mut moves = Vec::new();

    // Rule 1: the assistive lane keeps its stored vendor; formatting was OpenAI.
    let assistive_vendor = legacy
        .assistive_provider
        .as_deref()
        .and_then(|value| value.parse::<ProviderKind>().ok())
        .unwrap_or_default();
    let lanes = [
        (
            RuntimeLlmLaneKind::Formatting,
            ProviderKind::OpenAiResponses,
            legacy.formatting_endpoint.as_deref(),
            legacy.formatting_model.as_deref(),
        ),
        (
            RuntimeLlmLaneKind::Assistive,
            assistive_vendor,
            legacy.assistive_endpoint.as_deref(),
            legacy.assistive_model.as_deref(),
        ),
    ];

    for (lane, legacy_provider, lane_endpoint, lane_model) in lanes {
        // Rules 2–3: lane value, then the shared `speech.llm_*` fallback.
        let endpoint = lane_endpoint.or(legacy.llm_endpoint.as_deref());
        let model = lane_model.or(legacy.llm_model.as_deref());
        // Rule 4: resolve to vendor or custom row.
        let reference = resolve_legacy_endpoint(legacy_provider, endpoint, settings);
        let key_account = key_account_for(&reference, settings);
        let model = model
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        match lane {
            RuntimeLlmLaneKind::Formatting => {
                settings.llm_formatting_provider = Some(reference.as_string());
                settings.llm_formatting_model = model;
            }
            RuntimeLlmLaneKind::Assistive => {
                settings.llm_assistive_provider = Some(reference.as_string());
                settings.llm_assistive_model = model;
            }
        }
        // Rule 5 (§B.3): per-lane key move into the resolved provider account.
        moves.push(KeyMove {
            from: legacy_lane_account(lane).to_string(),
            to: key_account.clone(),
        });
        info!(
            lane = lane.as_str(),
            provider = %reference,
            model = settings_model(settings, lane).unwrap_or("<none>"),
            "migrated legacy LLM lane"
        );
    }

    // Rule 5 (§B.3): the shared `LLM_API_KEY` follows the shared endpoint's
    // host; no endpoint means OpenAI.
    let shared = legacy
        .llm_endpoint
        .as_deref()
        .map(|endpoint| {
            resolve_legacy_endpoint(ProviderKind::OpenAiResponses, Some(endpoint), settings)
        })
        .unwrap_or_default();
    moves.push(KeyMove {
        from: "LLM_API_KEY".to_string(),
        to: key_account_for(&shared, settings),
    });
    moves
}

/// The Keychain account a reference resolves to; a Custom id that is not in
/// `settings` (cannot happen after `resolve_legacy_endpoint`) falls back to
/// the OpenAI account rather than inventing one.
fn key_account_for(reference: &ProviderRef, settings: &UserSettings) -> String {
    match reference {
        ProviderRef::Vendor(kind) => kind.api_key_account().to_string(),
        ProviderRef::Custom(id) => settings
            .llm_custom_providers
            .iter()
            .find(|row| &row.id == id)
            .map(CustomProvider::key_account)
            .unwrap_or_else(|| ProviderKind::default().api_key_account().to_string()),
    }
}

/// The migrated model for `lane`, for the summary line.
fn settings_model(settings: &UserSettings, lane: RuntimeLlmLaneKind) -> Option<&str> {
    match lane {
        RuntimeLlmLaneKind::Formatting => settings.llm_formatting_model.as_deref(),
        RuntimeLlmLaneKind::Assistive => settings.llm_assistive_model.as_deref(),
    }
}

/// Fixture-driven witnesses for the two real shapes on the Founder's machines.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::WireFamily;

    /// The div0 `settings.json` (2026-09-07): every lane on the Libraxis gateway
    /// under an `openai-responses` provider label. Both lanes resolve to the
    /// Libraxis vendor, no Custom row, and every legacy key moves into the
    /// Libraxis account.
    fn div0_legacy() -> SpeechV2Legacy {
        SpeechV2Legacy {
            llm_endpoint: Some("https://api.libraxis.com/v1/responses".to_string()),
            llm_model: Some("buddy".to_string()),
            formatting_endpoint: None,
            formatting_model: Some("buddy".to_string()),
            assistive_endpoint: Some("https://api.libraxis.com/v1/responses".to_string()),
            assistive_model: Some("buddy".to_string()),
            assistive_provider: Some("openai-responses".to_string()),
        }
    }

    #[test]
    fn div0_fixture_lands_on_the_libraxis_vendor_for_both_lanes() {
        let legacy = div0_legacy();
        assert!(legacy.needs_migration());
        let mut settings = UserSettings::default();
        let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);

        assert_eq!(
            settings.llm_formatting_provider.as_deref(),
            Some("libraxis-responses")
        );
        assert_eq!(settings.llm_formatting_model.as_deref(), Some("buddy"));
        assert_eq!(
            settings.llm_assistive_provider.as_deref(),
            Some("libraxis-responses")
        );
        assert_eq!(settings.llm_assistive_model.as_deref(), Some("buddy"));
        assert!(
            settings.llm_custom_providers.is_empty(),
            "no custom row for a vendor host"
        );
        assert_eq!(
            moves,
            vec![
                KeyMove {
                    from: "LLM_FORMATTING_API_KEY".to_string(),
                    to: "LLM_LIBRAXIS_API_KEY".to_string(),
                },
                KeyMove {
                    from: "LLM_ASSISTIVE_API_KEY".to_string(),
                    to: "LLM_LIBRAXIS_API_KEY".to_string(),
                },
                KeyMove {
                    from: "LLM_API_KEY".to_string(),
                    to: "LLM_LIBRAXIS_API_KEY".to_string(),
                },
            ]
        );
    }

    /// The historical `.cloud` host is the same vendor.
    #[test]
    fn libraxis_cloud_host_is_the_same_vendor() {
        let legacy = SpeechV2Legacy {
            assistive_endpoint: Some("https://api.libraxis.cloud/v1/responses".to_string()),
            ..SpeechV2Legacy::default()
        };
        let mut settings = UserSettings::default();
        let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);
        assert_eq!(
            settings.llm_assistive_provider.as_deref(),
            Some("libraxis-responses")
        );
        // Formatting had no endpoint at all → stays on the OpenAI vendor.
        assert_eq!(
            settings.llm_formatting_provider.as_deref(),
            Some("openai-responses")
        );
        assert_eq!(moves[0].to, "LLM_OPENAI_API_KEY");
        assert_eq!(moves[1].to, "LLM_LIBRAXIS_API_KEY");
        assert_eq!(
            moves[2].to, "LLM_OPENAI_API_KEY",
            "no shared endpoint → OpenAI"
        );
    }

    /// A self-hosted assistive endpoint becomes one Custom row named after
    /// its host, on the legacy provider's wire, with the endpoint normalized;
    /// its key moves into the row's own account.
    #[test]
    fn self_hosted_endpoint_becomes_a_custom_row() {
        let legacy = SpeechV2Legacy {
            assistive_endpoint: Some("https://my.local:8080/v1/responses".to_string()),
            assistive_model: Some("qwen".to_string()),
            assistive_provider: Some("openai-responses".to_string()),
            ..SpeechV2Legacy::default()
        };
        let mut settings = UserSettings::default();
        let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);

        assert_eq!(settings.llm_custom_providers.len(), 1);
        let row = &settings.llm_custom_providers[0];
        assert_eq!(row.id, "my-local");
        assert_eq!(row.name, "my.local");
        assert_eq!(row.wire, WireFamily::OpenAiResponses);
        assert_eq!(row.endpoint, "https://my.local:8080/v1/responses");
        assert_eq!(
            settings.llm_assistive_provider.as_deref(),
            Some("custom:my-local")
        );
        assert_eq!(settings.llm_assistive_model.as_deref(), Some("qwen"));
        assert_eq!(moves[1].to, "LLM_CUSTOM_MY_LOCAL_API_KEY");
    }

    /// Both lanes on the same unknown host share one row; a bare `/v1` legacy
    /// endpoint is normalized to the wire's canonical path.
    #[test]
    fn two_lanes_on_one_unknown_host_share_a_single_row() {
        let legacy = SpeechV2Legacy {
            llm_endpoint: Some("http://127.0.0.1:1234/v1".to_string()),
            llm_model: Some("local".to_string()),
            ..SpeechV2Legacy::default()
        };
        let mut settings = UserSettings::default();
        let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);
        assert_eq!(settings.llm_custom_providers.len(), 1);
        assert_eq!(settings.llm_custom_providers[0].id, "127-0-0-1");
        assert_eq!(
            settings.llm_custom_providers[0].endpoint,
            "http://127.0.0.1:1234/v1/responses"
        );
        assert_eq!(
            settings.llm_formatting_provider.as_deref(),
            Some("custom:127-0-0-1")
        );
        assert_eq!(
            settings.llm_assistive_provider.as_deref(),
            Some("custom:127-0-0-1")
        );
        assert!(moves.iter().all(|m| m.to == "LLM_CUSTOM_127_0_0_1_API_KEY"));
    }

    #[test]
    fn distinct_services_on_one_host_keep_their_endpoints_and_key_accounts() {
        for second_endpoint in [
            "http://localhost:9000/v1/responses",
            "http://localhost:8000/second/v1/responses",
        ] {
            let legacy = SpeechV2Legacy {
                formatting_endpoint: Some("http://localhost:8000/v1/responses".into()),
                assistive_endpoint: Some(second_endpoint.into()),
                ..SpeechV2Legacy::default()
            };
            let mut settings = UserSettings::default();
            let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);
            assert_eq!(settings.llm_custom_providers.len(), 2);
            assert_ne!(
                settings.llm_formatting_provider,
                settings.llm_assistive_provider
            );
            assert_eq!(
                settings.llm_custom_providers[0].endpoint,
                "http://localhost:8000/v1/responses"
            );
            assert_eq!(settings.llm_custom_providers[1].endpoint, second_endpoint);
            assert_ne!(moves[0].to, moves[1].to);
        }
    }

    /// A legacy Anthropic assistive lane on the vendor host keeps its vendor
    /// and takes the vendor's key account.
    #[test]
    fn anthropic_assistive_lane_keeps_its_vendor() {
        let legacy = SpeechV2Legacy {
            assistive_endpoint: Some("https://api.anthropic.com/v1/messages".to_string()),
            assistive_model: Some("claude-sonnet-5".to_string()),
            assistive_provider: Some("anthropic-messages".to_string()),
            ..SpeechV2Legacy::default()
        };
        let mut settings = UserSettings::default();
        let moves = migrate_legacy_llm_lanes(&legacy, &mut settings);
        assert_eq!(
            settings.llm_assistive_provider.as_deref(),
            Some("anthropic-messages")
        );
        assert_eq!(moves[1].to, "LLM_ANTHROPIC_API_KEY");
    }

    /// Second pass: the migrated document carries only current-schema fields
    /// (per-lane model + provider), so the trigger is false and the queue is
    /// untouched. The trigger reads V2 and flat V1 spellings alike.
    #[test]
    fn a_second_pass_has_nothing_to_migrate() {
        let migrated = serde_json::json!({
            "schema_version": 3,
            "speech": {
                "formatting": { "llm_provider": "libraxis-responses", "llm_model": "buddy" },
                "assistive": { "provider": "libraxis-responses", "llm_model": "buddy" }
            }
        });
        assert!(!SpeechV2Legacy::from_json(&migrated).needs_migration());
        let v1 = serde_json::json!({ "llm_endpoint": "https://api.libraxis.com/v1/responses" });
        let legacy = SpeechV2Legacy::from_json(&v1);
        assert!(legacy.needs_migration());
        assert_eq!(
            legacy.llm_endpoint.as_deref(),
            Some("https://api.libraxis.com/v1/responses")
        );
        let blank = serde_json::json!({ "speech": { "llm_endpoint": "  " } });
        assert!(!SpeechV2Legacy::from_json(&blank).needs_migration());
    }
}
