//! Canonical LLM provider identity and per-model capability policy.
//!
//! This module is the single source of truth for *which* LLM wire protocol a
//! request targets ([`ProviderKind`]) and *what* that protocol will accept for a
//! given model ([`CapabilityPolicy`]). It exists because OpenAI Responses and
//! Anthropic Messages disagree on request shape — and, critically, because two
//! Anthropic models disagree *with each other*:
//!
//! - `claude-opus-4-8` (assistive) rejects `temperature`/`top_p`/`top_k` and a
//!   manual `thinking.budget_tokens` with HTTP 400.
//! - `claude-sonnet-4-6` (formatting) still accepts `temperature` and only
//!   *deprecates* `budget_tokens` (not a hard 400).
//!
//! Encoding that asymmetry here keeps the request builders (OpenAI today,
//! Anthropic in W2/W3) from sharing unsafe assumptions. This layer is pure data +
//! parsing: it performs **no** HTTP and holds **no** provider implementation.
//!
//! OpenAI is the default everywhere. Nothing in this module changes the OpenAI
//! request path — [`capability_policy`] returns a permissive policy for
//! [`ProviderKind::OpenAiResponses`] so the existing Responses builder keeps
//! sending `temperature` and using `previous_response_id` exactly as before.

use std::str::FromStr;

use crate::llm::account_auth;

use tracing::warn;

/// Canonical LLM provider identity — the wire protocol a request targets.
///
/// The string forms are the stable on-the-wire / env-var spellings. New
/// providers append variants here; the request layer branches on this enum
/// rather than sniffing endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// OpenAI Responses API (`/v1/responses`). The default.
    OpenAiResponses,
    /// Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
}

impl ProviderKind {
    /// Canonical lowercase-kebab spelling used in env vars and persisted config.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAiResponses => "openai-responses",
            ProviderKind::AnthropicMessages => "anthropic-messages",
        }
    }

    /// Human-readable label for provider pickers (Settings UI).
    pub const fn display_name(self) -> &'static str {
        match self {
            ProviderKind::OpenAiResponses => "OpenAI (Responses)",
            ProviderKind::AnthropicMessages => "Anthropic (Messages)",
        }
    }

    /// Env var / Keychain account holding the assistive-lane API key for this
    /// provider. OpenAI shares the assistive-lane key; Anthropic has its own so
    /// the two secrets coexist and switching providers never overwrites a key.
    pub const fn api_key_env_key(self) -> &'static str {
        match self {
            ProviderKind::OpenAiResponses => "LLM_ASSISTIVE_API_KEY",
            ProviderKind::AnthropicMessages => "LLM_ANTHROPIC_API_KEY",
        }
    }
}

/// How a provider authenticates requests for a lane.
///
/// `ApiKey` is the default and preserves the existing request builders. The
/// provider-account path is an explicit opt-in foundation for future ChatGPT
/// sign-in; it does not change any caller until a request path chooses this
/// mode and asks for a bearer header.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    #[default]
    ApiKey,
    ProviderAccount,
}

impl AuthMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMode::ApiKey => "api-key",
            AuthMode::ProviderAccount => "provider-account",
        }
    }
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseAuthModeError(pub String);

impl std::fmt::Display for ParseAuthModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown auth mode '{}' (expected 'api-key' or 'provider-account')",
            self.0
        )
    }
}

impl std::error::Error for ParseAuthModeError {}

impl FromStr for AuthMode {
    type Err = ParseAuthModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "api-key" | "api_key" | "apikey" | "key" => Ok(AuthMode::ApiKey),
            "provider-account" | "provider_account" | "account" | "chatgpt" => {
                Ok(AuthMode::ProviderAccount)
            }
            other => Err(ParseAuthModeError(other.to_string())),
        }
    }
}

/// Every provider identity, in Settings-picker order. The request layer branches
/// on [`ProviderKind`]; Settings discovers model options via live provider APIs.
pub const ALL_PROVIDERS: [ProviderKind; 2] = [
    ProviderKind::OpenAiResponses,
    ProviderKind::AnthropicMessages,
];

impl Default for ProviderKind {
    /// OpenAI Responses is the default provider — never regress this without a
    /// test that explicitly configures another provider.
    fn default() -> Self {
        ProviderKind::OpenAiResponses
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a provider string cannot be mapped to a [`ProviderKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseProviderError(pub String);

impl std::fmt::Display for ParseProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown LLM provider '{}' (expected 'openai-responses' or 'anthropic-messages')",
            self.0
        )
    }
}

impl std::error::Error for ParseProviderError {}

impl FromStr for ProviderKind {
    type Err = ParseProviderError;

    /// Parse a provider identity. Case-insensitive, surrounding whitespace
    /// trimmed. Accepts the canonical kebab spelling plus the bare vendor name
    /// as a friendly alias. Anything else is an error (callers decide whether to
    /// fall back to the default — see [`resolve_provider`]).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai-responses" | "openai" | "openai_responses" => Ok(ProviderKind::OpenAiResponses),
            "anthropic-messages" | "anthropic" | "anthropic_messages" => {
                Ok(ProviderKind::AnthropicMessages)
            }
            other => Err(ParseProviderError(other.to_string())),
        }
    }
}

/// How a provider/model treats a manual thinking-budget (`budget_tokens`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetTokensPolicy {
    /// Sending `budget_tokens` is a hard HTTP 400 — omit it entirely and use
    /// adaptive thinking instead (`claude-opus-4-8`).
    Hard400,
    /// `budget_tokens` is deprecated but still functional as a transitional
    /// escape hatch (`claude-sonnet-4-6`). Prefer adaptive thinking.
    Deprecated,
    /// The concept does not apply / imposes no restriction from this policy
    /// (OpenAI Responses).
    NotApplicable,
}

/// Per-`(provider, model)` request capability policy.
///
/// The request builder consults this before emitting a request so it never sends
/// a parameter the target will reject. Booleans are "is this allowed / relevant
/// for this model"; they are intentionally coarse — value-level granularity
/// (e.g. which `effort` tiers exist) is the builder's concern, not this gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityPolicy {
    /// Whether non-default sampling params (`temperature`/`top_p`/`top_k`) may be
    /// sent. `false` ⇒ omit them (Opus-4.8 rejects with 400).
    pub allow_sampling_params: bool,
    /// How a manual `budget_tokens` is treated.
    pub budget_tokens: BudgetTokensPolicy,
    /// Whether Anthropic adaptive thinking (`thinking:{type:"adaptive"}`) is a
    /// supported request shape for this model. OpenAI: `false` (different concept).
    pub adaptive_thinking: bool,
    /// Whether `output_config.effort` is supported.
    pub effort: bool,
    /// Whether `refusal` arrives as a `stop_reason` on a successful HTTP 200 and
    /// must be branched on before reading content (Anthropic). OpenAI: `false`.
    pub refusal_stop_reason: bool,
    /// Whether the provider supports server-side conversation chaining via a
    /// `previous_response_id` (OpenAI Responses). Anthropic replays messages, so
    /// `false`.
    pub previous_response_id: bool,
    /// Whether this `(provider, model)` accepts image (vision) input blocks.
    /// `false` ⇒ the send path must surface a readable error instead of silently
    /// dropping attached images. Unknown Anthropic models default to the current
    /// vision-capable policy; this flag is the honest seam for a future text-only
    /// model family.
    pub supports_vision: bool,
}

impl CapabilityPolicy {
    /// Sanitize a requested temperature against this policy: returns the value
    /// only when sampling params are allowed, otherwise `None` (omit the param).
    ///
    /// This is the seam W2/W3 call when building an Anthropic request so a
    /// non-default `temperature` never reaches an Opus-4.8 send.
    pub fn sanitize_temperature(&self, requested: Option<f32>) -> Option<f32> {
        if self.allow_sampling_params {
            requested
        } else {
            None
        }
    }
}

/// Model-family classification used to pick an Anthropic capability policy.
///
/// CORRECTION.md pins behaviour for `claude-opus-4-8` and `claude-sonnet-4-6`
/// specifically; we generalise conservatively by family so a future
/// `opus-4-9`/`sonnet-4-7` inherits the safe shape rather than the permissive
/// one. An unrecognised Anthropic model falls back to the strict (Opus) policy —
/// omitting sampling params can never cause a 400, sending them can.
fn anthropic_policy_for_model(model: &str) -> CapabilityPolicy {
    let m = model.to_ascii_lowercase();
    if m.contains("sonnet") {
        // claude-sonnet-4-6 (formatting): tolerates temperature; budget_tokens
        // deprecated (not a hard 400).
        CapabilityPolicy {
            allow_sampling_params: true,
            budget_tokens: BudgetTokensPolicy::Deprecated,
            adaptive_thinking: true,
            effort: true,
            refusal_stop_reason: true,
            previous_response_id: false,
            supports_vision: true,
        }
    } else {
        // claude-opus-4-8 (assistive) and unknown Anthropic models: strict.
        // Sampling params → 400; manual budget_tokens → 400.
        CapabilityPolicy {
            allow_sampling_params: false,
            budget_tokens: BudgetTokensPolicy::Hard400,
            adaptive_thinking: true,
            effort: true,
            refusal_stop_reason: true,
            previous_response_id: false,
            supports_vision: true,
        }
    }
}

/// The permissive OpenAI Responses policy. Kept in one place so it is obvious
/// that the OpenAI request path is unchanged by this layer.
const fn openai_policy() -> CapabilityPolicy {
    CapabilityPolicy {
        allow_sampling_params: true,
        budget_tokens: BudgetTokensPolicy::NotApplicable,
        adaptive_thinking: false,
        effort: true,
        refusal_stop_reason: false,
        previous_response_id: true,
        supports_vision: true,
    }
}

/// Whether the given `(provider, model)` accepts image (vision) input. Thin
/// accessor over [`capability_policy`] for send paths that only need the vision
/// gate (e.g. the composer-attachment bridge). Keeps the vision decision in the
/// capability layer rather than duplicated at the FFI boundary.
pub fn provider_supports_vision(provider: ProviderKind, model: &str) -> bool {
    capability_policy(provider, model).supports_vision
}

/// Resolve the capability policy for a `(provider, model)` pair.
///
/// This is the per-model matrix from CORRECTION.md. OpenAI ignores `model` (its
/// policy is uniform and permissive); Anthropic branches on model family.
pub fn capability_policy(provider: ProviderKind, model: &str) -> CapabilityPolicy {
    match provider {
        ProviderKind::OpenAiResponses => openai_policy(),
        ProviderKind::AnthropicMessages => anthropic_policy_for_model(model),
    }
}

/// Which formatting/assistive lane a provider value is being resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMode {
    /// Fast/cheap formatting path (`LLM_FORMATTING_PROVIDER`).
    Formatting,
    /// Assistive / agent path (`LLM_ASSISTIVE_PROVIDER`).
    Assistive,
}

impl LlmMode {
    /// The env var carrying the provider identity for this lane.
    pub const fn provider_env_key(self) -> &'static str {
        match self {
            LlmMode::Formatting => "LLM_FORMATTING_PROVIDER",
            LlmMode::Assistive => "LLM_ASSISTIVE_PROVIDER",
        }
    }

    /// The env var carrying the auth mode for this lane.
    pub const fn auth_mode_env_key(self) -> &'static str {
        match self {
            LlmMode::Formatting => "LLM_FORMATTING_AUTH_MODE",
            LlmMode::Assistive => "LLM_ASSISTIVE_AUTH_MODE",
        }
    }
}

/// Resolve the configured provider for a lane from process env, defaulting to
/// OpenAI.
///
/// An unset/empty value ⇒ [`ProviderKind::OpenAiResponses`]. An *invalid*
/// value is logged and also falls back to OpenAI — misconfiguration must never
/// silently route to an unintended provider, and OpenAI is the protected
/// default. Callers wanting strict validation should use [`ProviderKind::from_str`]
/// directly.
pub fn resolve_provider(mode: LlmMode) -> ProviderKind {
    let key = mode.provider_env_key();
    match std::env::var(key) {
        Ok(raw) if !raw.trim().is_empty() => match ProviderKind::from_str(&raw) {
            Ok(kind) => kind,
            Err(e) => {
                warn!("{key}: {e}; falling back to {}", ProviderKind::default());
                ProviderKind::default()
            }
        },
        _ => ProviderKind::default(),
    }
}

/// Resolve the configured auth mode for a lane from process env, defaulting to
/// API keys. Invalid values are logged and fall back to `ApiKey`, so account
/// auth can never become active by typo.
pub fn resolve_auth_mode(mode: LlmMode) -> AuthMode {
    let key = mode.auth_mode_env_key();
    match std::env::var(key) {
        Ok(raw) if !raw.trim().is_empty() => match AuthMode::from_str(&raw) {
            Ok(kind) => kind,
            Err(e) => {
                warn!("{key}: {e}; falling back to {}", AuthMode::default());
                AuthMode::default()
            }
        },
        _ => AuthMode::default(),
    }
}

/// Optional Authorization header for the provider-account path.
///
/// Request builders are intentionally unchanged in this wave. Future callers can
/// ask this helper for a bearer header when `AuthMode=ProviderAccount`; the
/// default `ApiKey` mode returns `Ok(None)` and preserves the current API-key
/// behavior exactly.
pub async fn provider_account_authorization_header(
    provider: ProviderKind,
    mode: LlmMode,
) -> Result<Option<String>, account_auth::AccountAuthError> {
    if resolve_auth_mode(mode) != AuthMode::ProviderAccount {
        return Ok(None);
    }
    account_auth::authorization_header(provider).await.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ---- identity defaults ----

    #[test]
    fn default_provider_is_openai() {
        assert_eq!(ProviderKind::default(), ProviderKind::OpenAiResponses);
        assert_eq!(ProviderKind::default().as_str(), "openai-responses");
        assert_eq!(AuthMode::default(), AuthMode::ApiKey);
    }

    #[test]
    fn as_str_roundtrips_through_from_str() {
        for kind in [
            ProviderKind::OpenAiResponses,
            ProviderKind::AnthropicMessages,
        ] {
            assert_eq!(ProviderKind::from_str(kind.as_str()), Ok(kind));
        }
    }

    // ---- provider parsing ----

    #[test]
    fn parses_canonical_and_alias_spellings() {
        assert_eq!(
            ProviderKind::from_str("openai-responses"),
            Ok(ProviderKind::OpenAiResponses)
        );
        assert_eq!(
            ProviderKind::from_str("  OpenAI  "),
            Ok(ProviderKind::OpenAiResponses)
        );
        assert_eq!(
            ProviderKind::from_str("anthropic-messages"),
            Ok(ProviderKind::AnthropicMessages)
        );
        assert_eq!(
            ProviderKind::from_str("ANTHROPIC"),
            Ok(ProviderKind::AnthropicMessages)
        );
    }

    #[test]
    fn invalid_provider_is_an_error() {
        let err = ProviderKind::from_str("gemini").unwrap_err();
        assert_eq!(err, ParseProviderError("gemini".to_string()));
        assert!(err.to_string().contains("gemini"));
    }

    #[test]
    fn parses_auth_mode_spellings() {
        assert_eq!(AuthMode::from_str("api-key"), Ok(AuthMode::ApiKey));
        assert_eq!(
            AuthMode::from_str("provider_account"),
            Ok(AuthMode::ProviderAccount)
        );
        assert!(AuthMode::from_str("oauth-ish").is_err());
    }

    // ---- per-model capability policy ----

    #[test]
    fn openai_policy_is_permissive_and_unchanged() {
        // The OpenAI request path must not be perturbed: sampling allowed,
        // previous_response_id chaining kept, no Anthropic-only concepts.
        let p = capability_policy(ProviderKind::OpenAiResponses, "gpt-5.5");
        assert!(p.allow_sampling_params);
        assert!(p.previous_response_id);
        assert!(!p.refusal_stop_reason);
        assert!(!p.adaptive_thinking);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::NotApplicable);
        assert!(
            p.supports_vision,
            "OpenAI Responses models accept image input by default"
        );
        // Model must not matter for OpenAI.
        assert_eq!(
            capability_policy(ProviderKind::OpenAiResponses, "gpt-4.1"),
            p
        );
    }

    #[test]
    fn opus_4_8_rejects_sampling_and_hard_400s_budget_tokens() {
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-opus-4-8");
        assert!(
            !p.allow_sampling_params,
            "Opus-4.8 rejects temperature/top_p/top_k"
        );
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Hard400);
        assert!(p.adaptive_thinking);
        assert!(p.effort);
        assert!(p.refusal_stop_reason);
        assert!(!p.previous_response_id);
        // A non-default temperature is stripped for Opus.
        assert_eq!(p.sanitize_temperature(Some(0.7)), None);
    }

    #[test]
    fn sonnet_4_6_tolerates_temperature_and_deprecates_budget_tokens() {
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-sonnet-4-6");
        assert!(p.allow_sampling_params, "Sonnet-4.6 tolerates temperature");
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Deprecated);
        assert!(p.adaptive_thinking);
        assert!(p.effort);
        assert!(p.refusal_stop_reason);
        assert!(!p.previous_response_id);
        // Temperature survives for Sonnet.
        assert_eq!(p.sanitize_temperature(Some(0.3)), Some(0.3));
    }

    #[test]
    fn unknown_anthropic_model_falls_back_to_strict_policy() {
        // Safety: omitting sampling can't 400; sending it can. Unknown ⇒ strict.
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-future-9");
        assert!(!p.allow_sampling_params);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Hard400);
    }

    // ---- provider identity (display / key account) ----

    #[test]
    fn every_provider_has_display_name_and_key_account() {
        for kind in ALL_PROVIDERS {
            assert!(!kind.display_name().is_empty());
            assert!(!kind.api_key_env_key().is_empty());
        }
    }

    #[test]
    fn anthropic_key_account_is_distinct_from_openai() {
        assert_eq!(
            ProviderKind::OpenAiResponses.api_key_env_key(),
            "LLM_ASSISTIVE_API_KEY"
        );
        assert_eq!(
            ProviderKind::AnthropicMessages.api_key_env_key(),
            "LLM_ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn default_and_unknown_models_are_vision_capable() {
        assert!(provider_supports_vision(
            ProviderKind::OpenAiResponses,
            "gpt-5.5"
        ));
        assert!(provider_supports_vision(
            ProviderKind::AnthropicMessages,
            "claude-opus-4-8"
        ));
        assert!(provider_supports_vision(
            ProviderKind::AnthropicMessages,
            "claude-future-9"
        ));
    }

    // ---- env resolution (serialized: mutates process env) ----

    #[test]
    #[serial]
    fn resolve_provider_defaults_to_openai_when_unset() {
        let prev_f = std::env::var("LLM_FORMATTING_PROVIDER").ok();
        let prev_a = std::env::var("LLM_ASSISTIVE_PROVIDER").ok();
        unsafe {
            std::env::remove_var("LLM_FORMATTING_PROVIDER");
            std::env::remove_var("LLM_ASSISTIVE_PROVIDER");
        }

        assert_eq!(
            resolve_provider(LlmMode::Formatting),
            ProviderKind::OpenAiResponses
        );
        assert_eq!(
            resolve_provider(LlmMode::Assistive),
            ProviderKind::OpenAiResponses
        );

        restore("LLM_FORMATTING_PROVIDER", prev_f);
        restore("LLM_ASSISTIVE_PROVIDER", prev_a);
    }

    #[test]
    #[serial]
    fn resolve_provider_reads_mode_specific_values() {
        let prev_f = std::env::var("LLM_FORMATTING_PROVIDER").ok();
        let prev_a = std::env::var("LLM_ASSISTIVE_PROVIDER").ok();
        unsafe {
            std::env::set_var("LLM_FORMATTING_PROVIDER", "openai-responses");
            std::env::set_var("LLM_ASSISTIVE_PROVIDER", "anthropic-messages");
        }

        assert_eq!(
            resolve_provider(LlmMode::Formatting),
            ProviderKind::OpenAiResponses
        );
        assert_eq!(
            resolve_provider(LlmMode::Assistive),
            ProviderKind::AnthropicMessages
        );

        restore("LLM_FORMATTING_PROVIDER", prev_f);
        restore("LLM_ASSISTIVE_PROVIDER", prev_a);
    }

    #[test]
    #[serial]
    fn resolve_provider_falls_back_to_openai_on_invalid() {
        let prev = std::env::var("LLM_ASSISTIVE_PROVIDER").ok();
        unsafe { std::env::set_var("LLM_ASSISTIVE_PROVIDER", "not-a-provider") };

        assert_eq!(
            resolve_provider(LlmMode::Assistive),
            ProviderKind::OpenAiResponses
        );

        restore("LLM_ASSISTIVE_PROVIDER", prev);
    }

    #[test]
    #[serial]
    fn resolve_auth_mode_defaults_to_api_key_when_unset() {
        let prev_f = std::env::var("LLM_FORMATTING_AUTH_MODE").ok();
        let prev_a = std::env::var("LLM_ASSISTIVE_AUTH_MODE").ok();
        unsafe {
            std::env::remove_var("LLM_FORMATTING_AUTH_MODE");
            std::env::remove_var("LLM_ASSISTIVE_AUTH_MODE");
        }

        assert_eq!(resolve_auth_mode(LlmMode::Formatting), AuthMode::ApiKey);
        assert_eq!(resolve_auth_mode(LlmMode::Assistive), AuthMode::ApiKey);

        restore("LLM_FORMATTING_AUTH_MODE", prev_f);
        restore("LLM_ASSISTIVE_AUTH_MODE", prev_a);
    }

    #[test]
    #[serial]
    fn resolve_auth_mode_reads_mode_specific_values_and_falls_back_on_invalid() {
        let prev_f = std::env::var("LLM_FORMATTING_AUTH_MODE").ok();
        let prev_a = std::env::var("LLM_ASSISTIVE_AUTH_MODE").ok();
        unsafe {
            std::env::set_var("LLM_FORMATTING_AUTH_MODE", "provider-account");
            std::env::set_var("LLM_ASSISTIVE_AUTH_MODE", "bad-mode");
        }

        assert_eq!(
            resolve_auth_mode(LlmMode::Formatting),
            AuthMode::ProviderAccount
        );
        assert_eq!(resolve_auth_mode(LlmMode::Assistive), AuthMode::ApiKey);

        restore("LLM_FORMATTING_AUTH_MODE", prev_f);
        restore("LLM_ASSISTIVE_AUTH_MODE", prev_a);
    }

    #[tokio::test]
    #[serial]
    async fn api_key_mode_returns_no_provider_account_header() {
        let prev = std::env::var("LLM_ASSISTIVE_AUTH_MODE").ok();
        unsafe { std::env::remove_var("LLM_ASSISTIVE_AUTH_MODE") };

        let header = provider_account_authorization_header(
            ProviderKind::OpenAiResponses,
            LlmMode::Assistive,
        )
        .await
        .unwrap();

        assert_eq!(header, None);
        restore("LLM_ASSISTIVE_AUTH_MODE", prev);
    }

    #[tokio::test]
    #[serial]
    async fn provider_account_mode_refreshes_expired_token_and_returns_bearer() {
        use crate::llm::account_auth::{
            AccountTokens, OPENAI_ACCOUNT_TOKENS_ACCOUNT, OPENAI_CLIENT_ID_ENV, OPENAI_ISSUER_ENV,
            load_account_tokens, store_account_tokens,
        };

        let prev_mode = std::env::var("LLM_ASSISTIVE_AUTH_MODE").ok();
        let prev_client = std::env::var(OPENAI_CLIENT_ID_ENV).ok();
        let prev_issuer = std::env::var(OPENAI_ISSUER_ENV).ok();
        let prev_disable = std::env::var("CODESCRIBE_DISABLE_KEYCHAIN").ok();
        let prev_tokens = std::env::var(OPENAI_ACCOUNT_TOKENS_ACCOUNT).ok();
        // Isolate the settings store: client_id resolution reads settings.json
        // first, and this test must not see an operator-configured client id.
        let prev_data_dir = std::env::var("CODESCRIBE_DATA_DIR").ok();
        let scratch_data_dir =
            std::env::temp_dir().join(format!("cs_provider_account_auth_{}", std::process::id()));
        std::fs::create_dir_all(&scratch_data_dir).expect("scratch settings dir");
        let mut server = mockito::Server::new_async().await;
        let _refresh = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".to_string(), "refresh_token".to_string()),
                mockito::Matcher::UrlEncoded("client_id".to_string(), "client".to_string()),
                mockito::Matcher::UrlEncoded(
                    "refresh_token".to_string(),
                    "old-refresh".to_string(),
                ),
            ]))
            .with_status(200)
            .with_body(
                r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;

        unsafe {
            std::env::set_var("CODESCRIBE_DISABLE_KEYCHAIN", "1");
            std::env::set_var("LLM_ASSISTIVE_AUTH_MODE", "provider-account");
            std::env::set_var(OPENAI_CLIENT_ID_ENV, "client");
            std::env::set_var(OPENAI_ISSUER_ENV, server.url());
            std::env::set_var("CODESCRIBE_DATA_DIR", &scratch_data_dir);
        }
        let expired = AccountTokens {
            provider: ProviderKind::OpenAiResponses.as_str().to_string(),
            access_token: "old-access".to_string(),
            refresh_token: Some("old-refresh".to_string()),
            id_token: None,
            token_type: "Bearer".to_string(),
            expires_at_unix: Some(0),
        };
        store_account_tokens(ProviderKind::OpenAiResponses, &expired).unwrap();

        let header = provider_account_authorization_header(
            ProviderKind::OpenAiResponses,
            LlmMode::Assistive,
        )
        .await
        .unwrap();

        assert_eq!(header.as_deref(), Some("Bearer new-access"));
        let stored = load_account_tokens(ProviderKind::OpenAiResponses).unwrap();
        assert_eq!(stored.access_token, "new-access");
        assert_eq!(stored.refresh_token.as_deref(), Some("new-refresh"));

        restore("LLM_ASSISTIVE_AUTH_MODE", prev_mode);
        restore(OPENAI_CLIENT_ID_ENV, prev_client);
        restore(OPENAI_ISSUER_ENV, prev_issuer);
        restore("CODESCRIBE_DISABLE_KEYCHAIN", prev_disable);
        restore(OPENAI_ACCOUNT_TOKENS_ACCOUNT, prev_tokens);
        restore("CODESCRIBE_DATA_DIR", prev_data_dir);
        let _ = std::fs::remove_dir_all(scratch_data_dir);
    }

    fn restore(key: &str, prev: Option<String>) {
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
