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

/// Canonical LLM provider identity — one variant per vendor product.
///
/// The variant is only a handle: every property of a provider lives in its
/// [`ProviderIdentity`] row in [`PROVIDER_REGISTRY`]. Adding a vendor is a row
/// plus a variant, never a new arm in `as_str`/`display_name`/`api_key_env_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// OpenAI Responses API (`/v1/responses`). The default.
    OpenAiResponses,
    /// Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
    /// xAI Grok, served over the OpenAI Responses protocol at `api.x.ai`.
    XaiResponses,
}

/// The request/response protocol a provider speaks.
///
/// Deliberately separate from [`ProviderKind`]: several vendors ship the same
/// wire protocol (xAI serves the OpenAI Responses shape), so request builders
/// and capability policy branch on the *family*, never on the vendor name.
/// Without this split, every protocol-compatible vendor would fork the request
/// layer for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireFamily {
    /// OpenAI Responses (`/v1/responses`, `previous_response_id` chaining).
    OpenAiResponses,
    /// Anthropic Messages (`/v1/messages`, adaptive thinking, refusal stops).
    AnthropicMessages,
}

/// Everything the identity layer knows about one provider, as data.
///
/// This is the row a new vendor adds. Keep it free of behaviour: OAuth details
/// live in `account_auth::ProviderOAuthConfig`, per-model request limits in
/// [`CapabilityPolicy`].
#[derive(Debug, Clone, Copy)]
pub struct ProviderIdentity {
    /// The enum handle this row describes.
    pub kind: ProviderKind,
    /// Canonical lowercase-kebab spelling used in env vars and persisted config.
    pub canonical: &'static str,
    /// Extra accepted spellings (already lowercased) for [`FromStr`]. The bare
    /// vendor name belongs here so `LLM_ASSISTIVE_PROVIDER=openai` keeps working.
    pub aliases: &'static [&'static str],
    /// Human-readable label for provider pickers (Settings UI).
    pub display_name: &'static str,
    /// Env var / Keychain account holding the assistive-lane API key. Every
    /// provider owns a distinct account so the secrets coexist and switching
    /// providers never overwrites a key.
    pub api_key_env_key: &'static str,
    /// Protocol this provider speaks — what request builders branch on.
    pub wire_family: WireFamily,
    /// Model-id prefixes this vendor serves. A configured model matching another
    /// row's prefix is refused for this provider, because sending it would 404
    /// on the wire and read to the user as a broken key.
    ///
    /// Empty ⇒ this is the **catch-all** row: it accepts every id no other row
    /// claims. OpenAI holds that position, which is why a future `o5-preview`
    /// works without this table learning about it.
    pub model_prefixes: &'static [&'static str],
    /// Env override for this provider's wire endpoint.
    pub endpoint_env: &'static str,
    /// Endpoint used when neither the env override nor — for the default
    /// provider only — the generic lane settings supply one.
    pub default_endpoint: &'static str,
    /// Seed model for the formatting lane before live discovery runs.
    pub formatting_model: &'static str,
    /// Seed model for the assistive lane before live discovery runs.
    pub assistive_model: &'static str,
}

/// Anthropic's own wire defaults. They live beside the row that uses them so a
/// vendor's endpoint and models are one block to read, not three files.
const DEFAULT_ANTHROPIC_MESSAGES_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// xAI serves the Responses protocol from its OpenAI-compatible base URL.
const DEFAULT_XAI_RESPONSES_ENDPOINT: &str = "https://api.x.ai/v1/responses";
/// Current Grok model at the time of this cut. Both lanes share it: it is only
/// a seed until Settings runs live discovery, and naming one verified id beats
/// guessing a cheaper one that may already be retired. An operator who wants a
/// lighter formatting model sets `LLM_FORMATTING_MODEL` after discovery.
const DEFAULT_XAI_MODEL: &str = "grok-4.5";

/// Registry row for OpenAI Responses — catch-all prefixes and default-lane seeds.
const OPENAI_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::OpenAiResponses,
    canonical: "openai-responses",
    aliases: &["openai", "openai_responses"],
    display_name: "OpenAI (Responses)",
    api_key_env_key: "LLM_ASSISTIVE_API_KEY",
    wire_family: WireFamily::OpenAiResponses,
    // Catch-all row: OpenAI serves every id no other vendor claims.
    model_prefixes: &[],
    endpoint_env: "LLM_ASSISTIVE_ENDPOINT",
    default_endpoint: crate::config::DEFAULT_OPENAI_RESPONSES_ENDPOINT,
    formatting_model: crate::config::DEFAULT_FORMATTING_MODEL,
    assistive_model: crate::config::DEFAULT_ASSISTIVE_MODEL,
};

/// Registry row for Anthropic Messages — claude prefixes and dual-lane seeds.
const ANTHROPIC_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::AnthropicMessages,
    canonical: "anthropic-messages",
    aliases: &["anthropic", "anthropic_messages"],
    display_name: "Anthropic (Messages)",
    api_key_env_key: "LLM_ANTHROPIC_API_KEY",
    wire_family: WireFamily::AnthropicMessages,
    model_prefixes: &["claude"],
    endpoint_env: "LLM_ANTHROPIC_ENDPOINT",
    default_endpoint: DEFAULT_ANTHROPIC_MESSAGES_ENDPOINT,
    formatting_model: "claude-sonnet-4-6",
    assistive_model: "claude-opus-4-8",
};

/// Registry row for xAI Grok on the Responses wire — grok prefixes, shared seeds.
const XAI_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::XaiResponses,
    canonical: "xai-responses",
    aliases: &["xai", "grok", "xai_responses"],
    display_name: "xAI (Grok)",
    api_key_env_key: "LLM_XAI_API_KEY",
    // Same protocol as OpenAI: request builders and capability policy reach xAI
    // through the wire family, so no send path learns the word "xai".
    wire_family: WireFamily::OpenAiResponses,
    model_prefixes: &["grok"],
    endpoint_env: "LLM_XAI_ENDPOINT",
    default_endpoint: DEFAULT_XAI_RESPONSES_ENDPOINT,
    formatting_model: DEFAULT_XAI_MODEL,
    assistive_model: DEFAULT_XAI_MODEL,
};

/// Every provider identity, in Settings-picker order. One row per vendor —
/// this array plus the matching `ProviderKind` variant is the whole cost of a
/// new provider at the identity layer.
pub const PROVIDER_REGISTRY: [ProviderIdentity; 3] =
    [OPENAI_IDENTITY, ANTHROPIC_IDENTITY, XAI_IDENTITY];

impl ProviderKind {
    /// This provider's registry row.
    pub const fn identity(self) -> &'static ProviderIdentity {
        match self {
            ProviderKind::OpenAiResponses => &OPENAI_IDENTITY,
            ProviderKind::AnthropicMessages => &ANTHROPIC_IDENTITY,
            ProviderKind::XaiResponses => &XAI_IDENTITY,
        }
    }

    /// Canonical lowercase-kebab spelling used in env vars and persisted config.
    pub const fn as_str(self) -> &'static str {
        self.identity().canonical
    }

    /// Human-readable label for provider pickers (Settings UI).
    pub const fn display_name(self) -> &'static str {
        self.identity().display_name
    }

    /// Env var / Keychain account holding the assistive-lane API key for this
    /// provider. OpenAI shares the assistive-lane key; Anthropic has its own so
    /// the two secrets coexist and switching providers never overwrites a key.
    pub const fn api_key_env_key(self) -> &'static str {
        self.identity().api_key_env_key
    }

    /// The protocol this provider speaks. Branch on this, not on the variant,
    /// whenever the question is "what shape does the request take".
    pub const fn wire_family(self) -> WireFamily {
        self.identity().wire_family
    }

    /// Whether `model` is an id this provider actually serves.
    ///
    /// A row that declares prefixes owns exactly those. The catch-all row (no
    /// prefixes) owns everything the other rows do not claim, so OpenAI keeps
    /// accepting unreleased ids while still refusing `claude-*` and `grok-*`.
    pub fn owns_model(self, model: &str) -> bool {
        let prefixes = self.identity().model_prefixes;
        if !prefixes.is_empty() {
            return prefixes.iter().any(|prefix| model.starts_with(prefix));
        }
        !PROVIDER_REGISTRY.iter().any(|row| {
            row.kind != self
                && row
                    .model_prefixes
                    .iter()
                    .any(|prefix| model.starts_with(prefix))
        })
    }

    /// This provider's seed model for `lane`, used until live discovery answers.
    pub const fn default_model(self, lane: LlmMode) -> &'static str {
        match lane {
            LlmMode::Formatting => self.identity().formatting_model,
            LlmMode::Assistive => self.identity().assistive_model,
        }
    }

    /// Whether the un-prefixed lane configuration (`LLM_ENDPOINT`, `LLM_MODEL`,
    /// `settings.llm_endpoint`, …) describes this provider.
    ///
    /// Those keys predate multi-provider support, so they mean the default
    /// provider and nothing else. This is a guard, not a formality: an operator
    /// whose `LLM_ENDPOINT` points at `api.openai.com` and who then switches the
    /// lane to Anthropic must not have Anthropic traffic sent to OpenAI's host.
    pub fn owns_generic_lane_config(self) -> bool {
        self == ProviderKind::default()
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
    /// Static API key from env or Keychain. The default and the only mode any
    /// request builder uses today.
    #[default]
    ApiKey,
    /// OAuth tokens obtained by signing in to the provider account, refreshed
    /// on demand by `account_auth`.
    ProviderAccount,
}

impl AuthMode {
    /// Canonical kebab spelling used in env vars and persisted config.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMode::ApiKey => "api-key",
            AuthMode::ProviderAccount => "provider-account",
        }
    }
}

impl std::fmt::Display for AuthMode {
    /// Write the canonical kebab spelling (`api-key` / `provider-account`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when an auth-mode string cannot be mapped to an [`AuthMode`].
/// Carries the normalised (trimmed, lowercased) input so the message names what
/// the operator actually configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseAuthModeError(pub String);

impl std::fmt::Display for ParseAuthModeError {
    /// Operator-facing message naming the rejected auth-mode string.
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
    /// Error type for unknown auth-mode spellings.
    type Err = ParseAuthModeError;

    /// Parse kebab/snake/bare aliases into [`AuthMode`]; unknown spellings err.
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

/// Every provider handle, in [`PROVIDER_REGISTRY`] order. Settings discovers
/// model options via live provider APIs. Kept as its own const because Rust has
/// no stable const `map`; `registry_and_all_providers_stay_in_lockstep` is the
/// test that keeps the two from drifting.
pub const ALL_PROVIDERS: [ProviderKind; PROVIDER_REGISTRY.len()] = [
    ProviderKind::OpenAiResponses,
    ProviderKind::AnthropicMessages,
    ProviderKind::XaiResponses,
];

impl Default for ProviderKind {
    /// OpenAI Responses is the default provider — never regress this without a
    /// test that explicitly configures another provider.
    fn default() -> Self {
        ProviderKind::OpenAiResponses
    }
}

impl std::fmt::Display for ProviderKind {
    /// Write the canonical registry spelling used in env and settings.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a provider string cannot be mapped to a [`ProviderKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseProviderError(pub String);

impl std::fmt::Display for ParseProviderError {
    /// Operator-facing message listing every accepted canonical spelling.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let expected = PROVIDER_REGISTRY
            .iter()
            .map(|row| format!("'{}'", row.canonical))
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "unknown LLM provider '{}' (expected {expected})", self.0)
    }
}

impl std::error::Error for ParseProviderError {}

impl FromStr for ProviderKind {
    /// Error type for unknown provider identity strings.
    type Err = ParseProviderError;

    /// Parse a provider identity against the registry. Case-insensitive,
    /// surrounding whitespace trimmed. Accepts each row's canonical kebab
    /// spelling plus its declared aliases. Anything else is an error (callers
    /// decide whether to fall back to the default — see [`resolve_provider`]).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        PROVIDER_REGISTRY
            .iter()
            .find(|row| row.canonical == needle || row.aliases.contains(&needle.as_str()))
            .map(|row| row.kind)
            .ok_or(ParseProviderError(needle))
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
/// This is the per-model matrix from CORRECTION.md, keyed by [`WireFamily`] so
/// a vendor that serves an existing protocol inherits its policy instead of
/// forking one. OpenAI Responses ignores `model` (its policy is uniform and
/// permissive); Anthropic Messages branches on model family.
pub fn capability_policy(provider: ProviderKind, model: &str) -> CapabilityPolicy {
    match provider.wire_family() {
        WireFamily::OpenAiResponses => openai_policy(),
        WireFamily::AnthropicMessages => anthropic_policy_for_model(model),
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

/// Unit tests pinning registry identity, capability policy, and env resolution.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ---- identity defaults ----

    /// OpenAI Responses is Default; AuthMode defaults to ApiKey.
    #[test]
    fn default_provider_is_openai() {
        assert_eq!(ProviderKind::default(), ProviderKind::OpenAiResponses);
        assert_eq!(ProviderKind::default().as_str(), "openai-responses");
        assert_eq!(AuthMode::default(), AuthMode::ApiKey);
    }

    /// Canonical `as_str` values parse back through `FromStr`.
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

    /// Accepts canonical kebab plus vendor aliases, case- and whitespace-tolerant.
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

    /// Unknown provider spellings surface `ParseProviderError`, not a silent pick.
    #[test]
    fn invalid_provider_is_an_error() {
        let err = ProviderKind::from_str("gemini").unwrap_err();
        assert_eq!(err, ParseProviderError("gemini".to_string()));
        assert!(err.to_string().contains("gemini"));
    }

    /// Auth-mode aliases map correctly; unknown values reject.
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

    /// OpenAI policy stays permissive so the Responses request path is untouched.
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

    /// Opus-4.8: no sampling params, hard-400 budget_tokens, strip temperature.
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

    /// Sonnet-4.6: temperature allowed; budget_tokens deprecated not hard-400.
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

    /// Unrecognised Anthropic models inherit the strict (Opus) policy.
    #[test]
    fn unknown_anthropic_model_falls_back_to_strict_policy() {
        // Safety: omitting sampling can't 400; sending it can. Unknown ⇒ strict.
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-future-9");
        assert!(!p.allow_sampling_params);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Hard400);
    }

    // ---- provider identity (display / key account) ----

    /// Every handle exposes a non-empty display label and key-account env name.
    #[test]
    fn every_provider_has_display_name_and_key_account() {
        for kind in ALL_PROVIDERS {
            assert!(!kind.display_name().is_empty());
            assert!(!kind.api_key_env_key().is_empty());
        }
    }

    /// `ALL_PROVIDERS` is hand-written (no stable const `map`), so it can only
    /// stay honest if a test pins it to the registry — order included, because
    /// Settings renders the picker in that order.
    #[test]
    fn registry_and_all_providers_stay_in_lockstep() {
        let from_registry: Vec<ProviderKind> =
            PROVIDER_REGISTRY.iter().map(|row| row.kind).collect();
        assert_eq!(from_registry, ALL_PROVIDERS.to_vec());
        for row in PROVIDER_REGISTRY {
            assert_eq!(
                row.kind.identity().canonical,
                row.canonical,
                "{} resolves to a different registry row than it declares",
                row.canonical
            );
        }
    }

    /// A new vendor copy-pasting a row is the realistic mistake: two providers
    /// sharing a spelling would make `from_str` pick one arbitrarily, and a
    /// shared key account would let one vendor read the other's secret.
    #[test]
    fn registry_rows_never_share_a_spelling_or_a_key_account() {
        let mut spellings: Vec<&str> = Vec::new();
        let mut accounts: Vec<&str> = Vec::new();
        let mut labels: Vec<&str> = Vec::new();
        for row in PROVIDER_REGISTRY {
            spellings.push(row.canonical);
            spellings.extend(row.aliases);
            accounts.push(row.api_key_env_key);
            labels.push(row.display_name);
        }
        for list in [&spellings, &accounts, &labels] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            assert_eq!(before, sorted.len(), "duplicate registry value in {list:?}");
        }
    }

    /// Every canonical and alias spelling resolves to its registry row.
    #[test]
    fn every_registry_spelling_parses_back_to_its_row() {
        for row in PROVIDER_REGISTRY {
            assert_eq!(ProviderKind::from_str(row.canonical), Ok(row.kind));
            for alias in row.aliases {
                assert_eq!(ProviderKind::from_str(alias), Ok(row.kind));
                assert_eq!(
                    ProviderKind::from_str(&alias.to_ascii_uppercase()),
                    Ok(row.kind)
                );
            }
        }
    }

    /// xAI is a registry row, not a fork: it speaks the Responses protocol, so
    /// every request builder must reach it through [`WireFamily`] and never
    /// through a vendor name. The spellings are pinned because they are written
    /// into `settings.json` and env by operators.
    #[test]
    fn xai_is_a_registry_row_on_the_responses_wire() {
        let xai = ProviderKind::XaiResponses;
        assert_eq!(xai.as_str(), "xai-responses");
        assert_eq!(ProviderKind::from_str("xai"), Ok(xai));
        assert_eq!(ProviderKind::from_str("grok"), Ok(xai));
        assert_eq!(xai.wire_family(), WireFamily::OpenAiResponses);
        assert_eq!(xai.api_key_env_key(), "LLM_XAI_API_KEY");
        assert_eq!(
            xai.identity().default_endpoint,
            "https://api.x.ai/v1/responses"
        );
        // Sharing the Responses wire must also hand xAI the Responses policy —
        // chaining via `previous_response_id`, no Anthropic refusal branch.
        let policy = capability_policy(xai, "grok-4.5");
        assert!(policy.previous_response_id);
        assert!(!policy.refusal_stop_reason);
    }

    /// A model id belongs to exactly one vendor. Without this, a lane switched
    /// to xAI would happily send `claude-opus-4-8` to `api.x.ai` (and the other
    /// way round) — a guaranteed 404 that looks like a broken key.
    #[test]
    fn model_prefixes_route_each_model_id_to_its_vendor() {
        use ProviderKind::*;
        assert!(AnthropicMessages.owns_model("claude-opus-4-8"));
        assert!(!AnthropicMessages.owns_model("gpt-5.5"));
        assert!(!AnthropicMessages.owns_model("grok-4.5"));

        assert!(XaiResponses.owns_model("grok-4.5"));
        assert!(!XaiResponses.owns_model("gpt-5.5"));
        assert!(!XaiResponses.owns_model("claude-opus-4-8"));

        // OpenAI is the catch-all row: it keeps accepting every id no other row
        // claims, so a future `o5-preview` needs no table edit.
        assert!(OpenAiResponses.owns_model("gpt-5.5"));
        assert!(OpenAiResponses.owns_model("o5-preview"));
        assert!(!OpenAiResponses.owns_model("claude-opus-4-8"));
        assert!(!OpenAiResponses.owns_model("grok-4.5"));
    }

    /// The un-prefixed `LLM_ENDPOINT` / `LLM_MODEL` keys predate multi-provider
    /// support, so they describe the default provider and nobody else. Letting a
    /// second vendor read them would point its traffic at an OpenAI host.
    #[test]
    fn generic_lane_config_belongs_to_the_default_provider() {
        for kind in ALL_PROVIDERS {
            assert_eq!(
                kind.owns_generic_lane_config(),
                kind == ProviderKind::default(),
                "{kind} disagrees with the default provider about the generic lane keys"
            );
        }
    }

    /// Every row must name a lane default it actually serves — a seed pointing
    /// at another vendor's model would 404 before live discovery ever runs.
    #[test]
    fn every_row_seeds_lane_defaults_it_owns() {
        for kind in ALL_PROVIDERS {
            for lane in [LlmMode::Formatting, LlmMode::Assistive] {
                let seed = kind.default_model(lane);
                assert!(!seed.is_empty(), "{kind} has no {lane:?} default model");
                assert!(
                    kind.owns_model(seed),
                    "{kind} seeds {lane:?} with '{seed}', which belongs to another vendor"
                );
            }
        }
    }

    /// Capability policy is keyed by wire family, so a future vendor speaking
    /// the Responses protocol inherits the OpenAI policy instead of forking it.
    #[test]
    fn capability_policy_follows_the_wire_family() {
        assert_eq!(
            ProviderKind::OpenAiResponses.wire_family(),
            WireFamily::OpenAiResponses
        );
        assert_eq!(
            ProviderKind::AnthropicMessages.wire_family(),
            WireFamily::AnthropicMessages
        );
        for row in PROVIDER_REGISTRY {
            let policy = capability_policy(row.kind, "claude-opus-4-8");
            match row.wire_family {
                WireFamily::OpenAiResponses => {
                    assert!(policy.previous_response_id);
                    assert!(!policy.refusal_stop_reason);
                }
                WireFamily::AnthropicMessages => {
                    assert!(!policy.previous_response_id);
                    assert!(policy.refusal_stop_reason);
                }
            }
        }
    }

    /// Anthropic keeps a separate Keychain/env account from OpenAI.
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

    /// Default OpenAI and Anthropic models (incl. unknown) accept vision input.
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

    /// Unset/empty provider env falls back to OpenAI for both lanes.
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

    /// Formatting and assistive provider env keys resolve independently.
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

    /// Invalid provider env logs and falls back to OpenAI, never another vendor.
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

    /// Unset auth-mode env defaults both lanes to ApiKey.
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

    /// Per-lane auth-mode env works; invalid values fall back to ApiKey.
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

    /// ApiKey mode yields no Authorization header from the account path.
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

    /// ProviderAccount mode refreshes an expired token and returns Bearer.
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
        let scratch_data_dir = tempfile::TempDir::new().expect("scratch settings dir");
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
            std::env::set_var("CODESCRIBE_DATA_DIR", scratch_data_dir.path());
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
    }

    /// Restore a process env var to its previous value (or remove if unset).
    fn restore(key: &str, prev: Option<String>) {
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
