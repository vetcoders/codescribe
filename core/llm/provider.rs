//! Canonical LLM provider registry and per-model capability policy.
//!
//! One provider model (ADR 2026-08-14, provider-registry-v2): a provider is
//! **credential + endpoint + wire**. Vendors ([`ProviderKind`]) have their
//! endpoint pinned in code; Custom providers ([`CustomProvider`]) carry theirs
//! in `settings.json`. A lane points at a [`ProviderRef`] and a model; the model
//! belongs to the lane, never to the provider, so no provider vetoes a model id.
//!
//! This layer is pure data + parsing: it performs **no** HTTP and holds **no**
//! provider implementation. Request builders branch on [`WireFamily`], never on
//! a vendor name, and consult [`capability_policy`] before emitting a request.

use std::borrow::Cow;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::config::UserSettings;
use crate::llm::account_auth;
use crate::llm::vendors;

/// Canonical LLM vendor identity — one variant per pinned vendor.
///
/// The variant is only a handle: every property of a vendor lives in its
/// registry row. Adding a vendor is a row plus a variant plus a `vendors::`
/// module, never a new arm in `as_str`/`display_name`/`api_key_account`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// OpenAI Responses API (`/v1/responses`). The default.
    OpenAiResponses,
    /// Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
    /// xAI Grok, served over the OpenAI Responses protocol at `api.x.ai`.
    XaiResponses,
    /// Libraxis gateway on the OpenAI Responses protocol at `api.libraxis.com`.
    /// Last in the picker until I1 promotes it on a live keyed witness (§B.3).
    LibraxisResponses,
}

/// The request/response protocol a provider speaks.
///
/// Deliberately separate from [`ProviderKind`]: several vendors ship the same
/// wire protocol (xAI serves the OpenAI Responses shape), so request builders
/// and capability policy branch on the *family*, never on the vendor name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WireFamily {
    /// OpenAI Responses (`/v1/responses`, `previous_response_id` chaining).
    #[serde(rename = "responses")]
    OpenAiResponses,
    /// Anthropic Messages (`/v1/messages`, adaptive thinking, refusal stops).
    #[serde(rename = "messages")]
    AnthropicMessages,
}

impl WireFamily {
    /// Canonical spelling persisted in `settings.json` (`providers.custom[].wire`).
    pub const fn as_str(self) -> &'static str {
        match self {
            WireFamily::OpenAiResponses => "responses",
            WireFamily::AnthropicMessages => "messages",
        }
    }

    /// Parse the canonical spelling or the vendor-flavoured aliases.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "responses" | "openai-responses" => Some(WireFamily::OpenAiResponses),
            "messages" | "anthropic-messages" => Some(WireFamily::AnthropicMessages),
            _ => None,
        }
    }

    /// The path every endpoint on this wire ends with.
    pub const fn canonical_suffix(self) -> &'static str {
        match self {
            WireFamily::OpenAiResponses => "/v1/responses",
            WireFamily::AnthropicMessages => "/v1/messages",
        }
    }

    /// Normalize an operator-provided base URL to this wire's canonical path:
    /// trim, strip a known protocol suffix (or a bare `/v1`), append the
    /// canonical one. Pure string work — no settings, env, or Keychain.
    pub fn normalize_endpoint(self, raw: &str) -> String {
        let known: &[&str] = match self {
            WireFamily::OpenAiResponses => {
                &["/v1/responses", "/v1/chat/completions", "/v1/completions"]
            }
            WireFamily::AnthropicMessages => &["/v1/messages", "/v1/responses"],
        };
        let mut base = raw.trim().trim_end_matches('/').to_string();
        for suffix in known {
            if base.ends_with(suffix) {
                base.truncate(base.len() - suffix.len());
                return format!("{base}{}", self.canonical_suffix());
            }
        }
        if base.ends_with("/v1") {
            base.truncate(base.len() - "/v1".len());
        }
        format!("{base}{}", self.canonical_suffix())
    }
}

impl std::fmt::Display for WireFamily {
    /// Write the canonical spelling (`responses` / `messages`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything the registry knows about one vendor, as data.
#[derive(Debug, Clone, Copy)]
struct ProviderIdentity {
    kind: ProviderKind,
    canonical: &'static str,
    aliases: &'static [&'static str],
    display_name: &'static str,
    api_key_account: &'static str,
    wire_family: WireFamily,
    /// Pinned. Never read from env or settings.
    endpoint: &'static str,
    /// Every host this vendor answers on; legacy endpoints on one of them
    /// migrate to the vendor row. Empty ⇒ only the endpoint's own host.
    extra_hosts: &'static [&'static str],
    formatting_model: &'static str,
    assistive_model: &'static str,
}

/// OpenAI Responses endpoint. Pinned here until I1 rewires the row to
/// `vendors::openai` (cut W1-OA).
pub const DEFAULT_OPENAI_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";
/// OpenAI formatting-lane seed until live discovery answers.
pub const DEFAULT_FORMATTING_MODEL: &str = "gpt-4.1";
/// OpenAI assistive-lane seed until live discovery answers.
pub const DEFAULT_ASSISTIVE_MODEL: &str = "gpt-5.5";
/// xAI serves the Responses protocol from its OpenAI-compatible base URL.
/// Pinned here until I1 rewires the row to `vendors::xai` (cut W1-XA).
const DEFAULT_XAI_RESPONSES_ENDPOINT: &str = "https://api.x.ai/v1/responses";
/// Current Grok model at the time of this cut; both lanes share the seed.
const DEFAULT_XAI_MODEL: &str = "grok-4.5";

const LIBRAXIS_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::LibraxisResponses,
    canonical: vendors::libraxis::CANONICAL,
    aliases: vendors::libraxis::ALIASES,
    display_name: vendors::libraxis::DISPLAY_NAME,
    api_key_account: vendors::libraxis::API_KEY_ACCOUNT,
    wire_family: WireFamily::OpenAiResponses,
    endpoint: vendors::libraxis::ENDPOINT,
    extra_hosts: vendors::libraxis::HOSTS,
    formatting_model: vendors::libraxis::DEFAULT_FORMATTING_MODEL,
    assistive_model: vendors::libraxis::DEFAULT_ASSISTIVE_MODEL,
};

const OPENAI_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::OpenAiResponses,
    canonical: "openai-responses",
    aliases: &["openai", "openai_responses"],
    display_name: "OpenAI (Responses)",
    api_key_account: "LLM_OPENAI_API_KEY",
    wire_family: WireFamily::OpenAiResponses,
    endpoint: DEFAULT_OPENAI_RESPONSES_ENDPOINT,
    extra_hosts: &[],
    formatting_model: DEFAULT_FORMATTING_MODEL,
    assistive_model: DEFAULT_ASSISTIVE_MODEL,
};

const XAI_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::XaiResponses,
    canonical: "xai-responses",
    aliases: &["xai", "grok", "xai_responses"],
    display_name: "xAI (Grok)",
    api_key_account: "LLM_XAI_API_KEY",
    wire_family: WireFamily::OpenAiResponses,
    endpoint: DEFAULT_XAI_RESPONSES_ENDPOINT,
    extra_hosts: &[],
    formatting_model: DEFAULT_XAI_MODEL,
    assistive_model: DEFAULT_XAI_MODEL,
};

const ANTHROPIC_IDENTITY: ProviderIdentity = ProviderIdentity {
    kind: ProviderKind::AnthropicMessages,
    canonical: vendors::anthropic::CANONICAL,
    aliases: vendors::anthropic::ALIASES,
    display_name: vendors::anthropic::DISPLAY_NAME,
    api_key_account: vendors::anthropic::API_KEY_ACCOUNT,
    wire_family: WireFamily::AnthropicMessages,
    endpoint: vendors::anthropic::ENDPOINT,
    extra_hosts: &[],
    formatting_model: vendors::anthropic::DEFAULT_FORMATTING_MODEL,
    assistive_model: vendors::anthropic::DEFAULT_ASSISTIVE_MODEL,
};

/// Every vendor row, in picker order.
const PROVIDER_REGISTRY: [ProviderIdentity; 4] = [
    LIBRAXIS_IDENTITY,
    OPENAI_IDENTITY,
    XAI_IDENTITY,
    ANTHROPIC_IDENTITY,
];

/// Every vendor handle, in picker order: Libraxis, OpenAI, xAI, Anthropic.
/// Libraxis earned the first slot on a live two-part witness at integration
/// (2026-09-07 18:16Z: `GET /v1/models` 200 with 8 aliases, one Responses call
/// completed); the lane default stays OpenAI (`ProviderKind::default`).
pub const ALL_PROVIDERS: [ProviderKind; 4] = [
    ProviderKind::LibraxisResponses,
    ProviderKind::OpenAiResponses,
    ProviderKind::XaiResponses,
    ProviderKind::AnthropicMessages,
];

impl ProviderKind {
    const fn identity(self) -> &'static ProviderIdentity {
        match self {
            ProviderKind::OpenAiResponses => &OPENAI_IDENTITY,
            ProviderKind::AnthropicMessages => &ANTHROPIC_IDENTITY,
            ProviderKind::XaiResponses => &XAI_IDENTITY,
            ProviderKind::LibraxisResponses => &LIBRAXIS_IDENTITY,
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

    /// The protocol this vendor speaks. Branch on this, not on the variant,
    /// whenever the question is "what shape does the request take".
    pub const fn wire_family(self) -> WireFamily {
        self.identity().wire_family
    }

    /// The vendor's pinned wire endpoint. There is no override: a different
    /// host is a [`CustomProvider`].
    pub const fn endpoint(self) -> &'static str {
        self.identity().endpoint
    }

    /// Keychain account holding this vendor's API key. Every vendor owns a
    /// distinct account so switching providers never overwrites a key.
    pub const fn api_key_account(self) -> &'static str {
        self.identity().api_key_account
    }

    /// This vendor's seed model for `lane`, used until live discovery answers.
    pub const fn default_model(self, lane: LlmMode) -> &'static str {
        match lane {
            LlmMode::Formatting => self.identity().formatting_model,
            LlmMode::Assistive => self.identity().assistive_model,
        }
    }

    /// Host of the pinned endpoint, for legacy endpoint → vendor migration.
    pub fn host(self) -> &'static str {
        endpoint_host(self.endpoint())
    }

    /// Whether `host` is one this vendor answers on (pinned host plus any
    /// extra hosts the vendor row declares, e.g. Libraxis' `.cloud`).
    pub fn matches_host(self, host: &str) -> bool {
        host.eq_ignore_ascii_case(self.host())
            || self
                .identity()
                .extra_hosts
                .iter()
                .any(|candidate| host.eq_ignore_ascii_case(candidate))
    }
}

/// Host component of a URL (`https://api.x.ai/v1/responses` → `api.x.ai`).
pub fn endpoint_host(endpoint: &str) -> &str {
    endpoint
        .split("://")
        .nth(1)
        .unwrap_or(endpoint)
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or_default()
}

impl Default for ProviderKind {
    /// OpenAI Responses is the default vendor — never regress this without a
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
    type Err = ParseProviderError;

    /// Parse a vendor identity against the registry. Case-insensitive,
    /// surrounding whitespace trimmed. Accepts each row's canonical kebab
    /// spelling plus its declared aliases.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        PROVIDER_REGISTRY
            .iter()
            .find(|row| row.canonical == needle || row.aliases.contains(&needle.as_str()))
            .map(|row| row.kind)
            .ok_or(ParseProviderError(needle))
    }
}

/// What a lane points at: a pinned vendor or a Custom provider row by id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProviderRef {
    /// One of [`ALL_PROVIDERS`].
    Vendor(ProviderKind),
    /// `settings.providers.custom[]` row, by its immutable slug id.
    Custom(String),
}

/// Prefix of a Custom provider reference as persisted in settings and env.
const CUSTOM_REF_PREFIX: &str = "custom:";

impl ProviderRef {
    /// Persisted spelling: the vendor's canonical id or `custom:<id>`.
    pub fn as_string(&self) -> String {
        self.as_str().into_owned()
    }

    /// Same as [`Self::as_string`] without allocating for vendors.
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            ProviderRef::Vendor(kind) => Cow::Borrowed(kind.as_str()),
            ProviderRef::Custom(id) => Cow::Owned(format!("{CUSTOM_REF_PREFIX}{id}")),
        }
    }

    /// Parse the persisted spelling. Vendor aliases are accepted; a custom id
    /// is taken verbatim after `custom:` (trimmed, lowercased) and must be a
    /// non-empty slug.
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if let Some(id) = trimmed
            .strip_prefix(CUSTOM_REF_PREFIX)
            .or_else(|| trimmed.strip_prefix("CUSTOM:"))
        {
            let id = id.trim().to_ascii_lowercase();
            return (!id.is_empty() && slug(&id) == id).then_some(ProviderRef::Custom(id));
        }
        ProviderKind::from_str(trimmed)
            .ok()
            .map(ProviderRef::Vendor)
    }

    /// The vendor, if this is a vendor reference.
    pub fn vendor(&self) -> Option<ProviderKind> {
        match self {
            ProviderRef::Vendor(kind) => Some(*kind),
            ProviderRef::Custom(_) => None,
        }
    }

    /// The custom row id, if this is a custom reference.
    pub fn custom_id(&self) -> Option<&str> {
        match self {
            ProviderRef::Vendor(_) => None,
            ProviderRef::Custom(id) => Some(id),
        }
    }
}

impl Default for ProviderRef {
    /// The default vendor, so a fresh install and a removed custom row both
    /// land on OpenAI.
    fn default() -> Self {
        ProviderRef::Vendor(ProviderKind::OpenAiResponses)
    }
}

impl std::fmt::Display for ProviderRef {
    /// Write the persisted spelling.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// An operator-defined provider: any host speaking a known wire.
///
/// The `id` is derived from the name once and never changes, because it is
/// the Keychain account suffix and the lane pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub wire: WireFamily,
    pub endpoint: String,
}

impl CustomProvider {
    /// Validate and build a row: `id = slug(name)` must be non-empty, the
    /// endpoint is normalized onto the wire's canonical path and must carry an
    /// `http(s)` scheme and a host.
    pub fn new(name: &str, wire: WireFamily, endpoint: &str) -> Result<Self, ProviderError> {
        let name = name.trim();
        let id = slug(name);
        if id.is_empty() {
            return Err(ProviderError::EmptyName);
        }
        let endpoint = wire.normalize_endpoint(endpoint);
        let scheme_ok = endpoint.starts_with("http://") || endpoint.starts_with("https://");
        if !scheme_ok || endpoint_host(&endpoint).is_empty() {
            return Err(ProviderError::InvalidEndpoint(endpoint));
        }
        Ok(Self {
            id,
            name: name.to_string(),
            wire,
            endpoint,
        })
    }

    /// Keychain account holding this row's API key.
    pub fn key_account(&self) -> String {
        custom_key_account(&self.id)
    }
}

/// Lowercase `[a-z0-9-]` slug: every other run of characters becomes one `-`,
/// leading/trailing dashes are dropped.
fn slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

/// Keychain account for a Custom provider id: `LLM_CUSTOM_<ID>_API_KEY` with
/// the slug uppercased and dashes turned into underscores.
pub fn custom_key_account(id: &str) -> String {
    format!(
        "LLM_CUSTOM_{}_API_KEY",
        id.to_ascii_uppercase().replace('-', "_")
    )
}

/// Whether an account name has the Custom provider shape.
pub fn is_custom_key_account(account: &str) -> bool {
    account
        .strip_prefix("LLM_CUSTOM_")
        .and_then(|rest| rest.strip_suffix("_API_KEY"))
        .is_some_and(|middle| !middle.is_empty())
}

/// Everything that can go wrong building or resolving a provider row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The name slugs to nothing (`""`, `"---"`, `"  "`).
    EmptyName,
    /// Missing `http(s)` scheme or host; carries the normalized endpoint.
    InvalidEndpoint(String),
    /// A row with this id already exists.
    DuplicateId(String),
    /// No vendor or custom row answers to this reference.
    UnknownProvider(String),
}

impl std::fmt::Display for ProviderError {
    /// Operator-facing sentence for Settings error rows.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::EmptyName => f.write_str("provider name must contain a letter or digit"),
            ProviderError::InvalidEndpoint(endpoint) => {
                write!(
                    f,
                    "endpoint '{endpoint}' needs an http(s) scheme and a host"
                )
            }
            ProviderError::DuplicateId(id) => {
                write!(f, "a custom provider with id '{id}' already exists")
            }
            ProviderError::UnknownProvider(reference) => {
                write!(f, "unknown provider '{reference}'")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

/// A [`ProviderRef`] resolved against the registry: everything a lane, a
/// probe, or model discovery needs, with no further lookups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub reference: ProviderRef,
    pub display_name: String,
    pub wire: WireFamily,
    pub endpoint: String,
    pub key_account: String,
    /// Vendors always need a key (or a signed-in account); a Custom host may
    /// be key-optional, so the lane stays available without one.
    pub key_required: bool,
    /// `Some` only for vendors with a row in the OAuth registry.
    pub oauth_vendor: Option<ProviderKind>,
}

impl ResolvedProvider {
    /// Whether `model` accepts image input on this provider. Vendors answer
    /// through [`capability_policy`]; a Custom host is permissive — it returns
    /// a real error, and guessing would silently drop attachments.
    pub fn supports_vision(&self, model: &str) -> bool {
        match self.reference.vendor() {
            Some(kind) => provider_supports_vision(kind, model),
            None => true,
        }
    }
}

/// The three pinned vendors plus the operator's Custom rows.
#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    custom: Vec<CustomProvider>,
}

impl ProviderRegistry {
    /// Build from custom rows in settings order.
    pub fn new(custom: Vec<CustomProvider>) -> Self {
        Self { custom }
    }

    /// Build from the persisted `providers.custom[]` rows.
    pub fn from_settings(settings: &UserSettings) -> Self {
        Self::new(settings.llm_custom_providers.clone())
    }

    /// Resolve a reference. `None` means a Custom id that no longer exists —
    /// the loader turns that into an unavailable lane, not a crash.
    pub fn resolve(&self, reference: &ProviderRef) -> Option<ResolvedProvider> {
        match reference {
            ProviderRef::Vendor(kind) => Some(Self::resolve_vendor(*kind)),
            ProviderRef::Custom(id) => self
                .custom
                .iter()
                .find(|row| row.id == *id)
                .map(Self::resolve_custom),
        }
    }

    /// Vendors in [`ALL_PROVIDERS`] order, then custom rows in settings order.
    pub fn all(&self) -> Vec<ResolvedProvider> {
        ALL_PROVIDERS
            .iter()
            .map(|kind| Self::resolve_vendor(*kind))
            .chain(self.custom.iter().map(Self::resolve_custom))
            .collect()
    }

    /// The custom rows this registry was built from.
    pub fn custom(&self) -> &[CustomProvider] {
        &self.custom
    }

    fn resolve_vendor(kind: ProviderKind) -> ResolvedProvider {
        ResolvedProvider {
            reference: ProviderRef::Vendor(kind),
            display_name: kind.display_name().to_string(),
            wire: kind.wire_family(),
            endpoint: kind.endpoint().to_string(),
            key_account: kind.api_key_account().to_string(),
            key_required: true,
            oauth_vendor: account_auth::provider_oauth_config(kind)
                .ok()
                .map(|row| row.provider),
        }
    }

    fn resolve_custom(row: &CustomProvider) -> ResolvedProvider {
        ResolvedProvider {
            reference: ProviderRef::Custom(row.id.clone()),
            display_name: row.name.clone(),
            wire: row.wire,
            endpoint: row.endpoint.clone(),
            key_account: row.key_account(),
            key_required: false,
            oauth_vendor: None,
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
    /// dropping attached images.
    pub supports_vision: bool,
}

impl CapabilityPolicy {
    /// Sanitize a requested temperature against this policy: returns the value
    /// only when sampling params are allowed, otherwise `None` (omit the param).
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

/// Whether the given `(vendor, model)` accepts image (vision) input. Thin
/// accessor over [`capability_policy`] for send paths that only need the vision
/// gate. Custom providers go through [`ResolvedProvider::supports_vision`].
pub fn provider_supports_vision(provider: ProviderKind, model: &str) -> bool {
    capability_policy(provider, model).supports_vision
}

/// Resolve the capability policy for a `(vendor, model)` pair, keyed by
/// [`WireFamily`] so a vendor that serves an existing protocol inherits its
/// policy instead of forking one.
pub fn capability_policy(provider: ProviderKind, model: &str) -> CapabilityPolicy {
    match provider.wire_family() {
        WireFamily::OpenAiResponses => openai_policy(),
        WireFamily::AnthropicMessages => anthropic_policy_for_model(model),
    }
}

/// Which formatting/assistive lane a provider value is being resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMode {
    /// Fast/cheap formatting path.
    Formatting,
    /// Assistive / agent path.
    Assistive,
}

impl LlmMode {
    /// The env var carrying the provider reference for this lane.
    pub const fn provider_env_key(self) -> &'static str {
        match self {
            LlmMode::Formatting => "LLM_FORMATTING_PROVIDER",
            LlmMode::Assistive => "LLM_ASSISTIVE_PROVIDER",
        }
    }

    /// The env var carrying the model id for this lane.
    pub const fn model_env_key(self) -> &'static str {
        match self {
            LlmMode::Formatting => "LLM_FORMATTING_MODEL",
            LlmMode::Assistive => "LLM_ASSISTIVE_MODEL",
        }
    }
}

/// Unit tests pinning registry identity, references, custom rows, and policy.
#[cfg(test)]
mod tests {
    use super::*;

    /// OpenAI Responses is the default vendor and the default reference.
    #[test]
    fn default_provider_is_openai() {
        assert_eq!(ProviderKind::default(), ProviderKind::OpenAiResponses);
        assert_eq!(
            ProviderRef::default(),
            ProviderRef::Vendor(ProviderKind::OpenAiResponses)
        );
        assert_eq!(ProviderRef::default().as_string(), "openai-responses");
    }

    /// Every canonical and alias spelling resolves to its registry row, and no
    /// two rows share a spelling, a key account, or a label.
    #[test]
    fn registry_spellings_and_accounts_are_unique_and_parse_back() {
        let mut spellings: Vec<&str> = Vec::new();
        let mut accounts: Vec<&str> = Vec::new();
        for row in PROVIDER_REGISTRY {
            assert_eq!(ProviderKind::from_str(row.canonical), Ok(row.kind));
            for alias in row.aliases {
                assert_eq!(
                    ProviderKind::from_str(&alias.to_ascii_uppercase()),
                    Ok(row.kind)
                );
            }
            spellings.push(row.canonical);
            spellings.extend(row.aliases);
            accounts.push(row.api_key_account);
        }
        for list in [spellings, accounts] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                list.len(),
                "duplicate registry value in {list:?}"
            );
        }
        assert_eq!(
            ProviderKind::from_str("gemini").unwrap_err(),
            ParseProviderError("gemini".to_string())
        );
    }

    /// The one assertion on picker order: `ALL_PROVIDERS` matches the row
    /// table and reads OpenAI, xAI, Anthropic, Libraxis (§B.3 correction);
    /// the lane default stays OpenAI.
    #[test]
    fn all_providers_follow_registry_order() {
        let from_registry: Vec<ProviderKind> =
            PROVIDER_REGISTRY.iter().map(|row| row.kind).collect();
        assert_eq!(from_registry, ALL_PROVIDERS.to_vec());
        assert_eq!(
            ALL_PROVIDERS,
            [
                ProviderKind::LibraxisResponses,
                ProviderKind::OpenAiResponses,
                ProviderKind::XaiResponses,
                ProviderKind::AnthropicMessages,
            ]
        );
    }

    /// Vendor endpoints are compile-time constants with a distinct key account
    /// each; the Anthropic row reads its vendor module.
    #[test]
    fn vendor_rows_pin_endpoint_account_and_host() {
        let libraxis = ProviderKind::LibraxisResponses;
        assert_eq!(libraxis.as_str(), "libraxis-responses");
        assert_eq!(ProviderKind::from_str("lbrx"), Ok(libraxis));
        assert_eq!(libraxis.endpoint(), "https://api.libraxis.com/v1/responses");
        assert_eq!(libraxis.api_key_account(), "LLM_LIBRAXIS_API_KEY");
        assert_eq!(libraxis.wire_family(), WireFamily::OpenAiResponses);
        assert_eq!(libraxis.default_model(LlmMode::Assistive), "buddy");
        assert!(libraxis.matches_host("api.libraxis.com"));
        assert!(libraxis.matches_host("API.libraxis.cloud"));
        assert!(!libraxis.matches_host("api.openai.com"));
        assert!(!ProviderKind::OpenAiResponses.matches_host("api.libraxis.cloud"));
        assert_eq!(
            ProviderKind::OpenAiResponses.endpoint(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            ProviderKind::OpenAiResponses.api_key_account(),
            "LLM_OPENAI_API_KEY"
        );
        assert_eq!(ProviderKind::OpenAiResponses.host(), "api.openai.com");
        assert_eq!(
            ProviderKind::XaiResponses.endpoint(),
            "https://api.x.ai/v1/responses"
        );
        assert_eq!(
            ProviderKind::XaiResponses.api_key_account(),
            "LLM_XAI_API_KEY"
        );
        assert_eq!(
            ProviderKind::XaiResponses.wire_family(),
            WireFamily::OpenAiResponses
        );
        assert_eq!(
            ProviderKind::AnthropicMessages.endpoint(),
            vendors::anthropic::ENDPOINT
        );
        assert_eq!(
            ProviderKind::AnthropicMessages.api_key_account(),
            "LLM_ANTHROPIC_API_KEY"
        );
        assert_eq!(ProviderKind::AnthropicMessages.host(), "api.anthropic.com");
        assert_eq!(
            ProviderKind::AnthropicMessages.default_model(LlmMode::Formatting),
            "claude-sonnet-5"
        );
        assert_eq!(
            ProviderKind::AnthropicMessages.default_model(LlmMode::Assistive),
            "claude-opus-5"
        );
        for kind in ALL_PROVIDERS {
            for lane in [LlmMode::Formatting, LlmMode::Assistive] {
                assert!(!kind.default_model(lane).is_empty());
            }
        }
    }

    /// Wire spellings round-trip and both normalizers land on the canonical path.
    #[test]
    fn wire_family_parses_and_normalizes_endpoints() {
        for wire in [WireFamily::OpenAiResponses, WireFamily::AnthropicMessages] {
            assert_eq!(WireFamily::parse(wire.as_str()), Some(wire));
        }
        assert_eq!(
            WireFamily::parse("openai-responses"),
            Some(WireFamily::OpenAiResponses)
        );
        assert_eq!(
            WireFamily::parse("Anthropic-Messages"),
            Some(WireFamily::AnthropicMessages)
        );
        assert_eq!(WireFamily::parse("grpc"), None);
        let responses = WireFamily::OpenAiResponses;
        assert_eq!(
            responses.normalize_endpoint("https://api.libraxis.com/v1/chat/completions/"),
            "https://api.libraxis.com/v1/responses"
        );
        assert_eq!(
            responses.normalize_endpoint(" http://localhost:8080/v1 "),
            "http://localhost:8080/v1/responses"
        );
        assert_eq!(
            responses.normalize_endpoint("http://localhost:8080"),
            "http://localhost:8080/v1/responses"
        );
        assert_eq!(
            WireFamily::AnthropicMessages.normalize_endpoint("https://proxy.example/v1/responses"),
            "https://proxy.example/v1/messages"
        );
        assert_eq!(
            serde_json::to_string(&WireFamily::AnthropicMessages).unwrap(),
            "\"messages\""
        );
    }

    /// `custom:<id>` and vendor spellings round-trip through `parse`.
    #[test]
    fn provider_ref_round_trips_through_its_persisted_spelling() {
        let custom = ProviderRef::Custom("api-libraxis-com".to_string());
        assert_eq!(custom.as_string(), "custom:api-libraxis-com");
        assert_eq!(custom.as_str(), Cow::<str>::Owned(custom.as_string()));
        assert_eq!(
            ProviderRef::parse(" custom:API-Libraxis-com "),
            Some(custom.clone())
        );
        assert_eq!(custom.custom_id(), Some("api-libraxis-com"));
        assert_eq!(custom.vendor(), None);
        let xai = ProviderRef::Vendor(ProviderKind::XaiResponses);
        assert_eq!(ProviderRef::parse("grok"), Some(xai.clone()));
        assert!(matches!(xai.as_str(), Cow::Borrowed("xai-responses")));
        assert_eq!(xai.vendor(), Some(ProviderKind::XaiResponses));
        assert_eq!(ProviderRef::parse("custom:"), None);
        assert_eq!(ProviderRef::parse("custom:not a slug"), None);
        assert_eq!(ProviderRef::parse("gemini"), None);
    }

    /// A custom row slugs its name, normalizes its endpoint, and refuses
    /// empty names or endpoints without scheme/host.
    #[test]
    fn custom_provider_slugs_name_and_validates_endpoint() {
        let row = CustomProvider::new(
            "api.libraxis.com",
            WireFamily::OpenAiResponses,
            "https://api.libraxis.com/v1/responses",
        )
        .unwrap();
        assert_eq!(row.id, "api-libraxis-com");
        assert_eq!(row.name, "api.libraxis.com");
        assert_eq!(row.endpoint, "https://api.libraxis.com/v1/responses");
        assert_eq!(row.key_account(), "LLM_CUSTOM_API_LIBRAXIS_COM_API_KEY");
        let local = CustomProvider::new(
            "  My Local  LLM ",
            WireFamily::OpenAiResponses,
            "http://localhost:8080",
        )
        .unwrap();
        assert_eq!(local.id, "my-local-llm");
        assert_eq!(local.endpoint, "http://localhost:8080/v1/responses");
        assert_eq!(
            CustomProvider::new("---", WireFamily::OpenAiResponses, "http://h"),
            Err(ProviderError::EmptyName)
        );
        assert_eq!(
            CustomProvider::new("x", WireFamily::OpenAiResponses, "localhost:8080"),
            Err(ProviderError::InvalidEndpoint(
                "localhost:8080/v1/responses".to_string()
            ))
        );
        assert_eq!(
            CustomProvider::new("x", WireFamily::AnthropicMessages, "https:///v1/messages"),
            Err(ProviderError::InvalidEndpoint(
                "https:///v1/messages".to_string()
            ))
        );
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["wire"], "responses");
        assert_eq!(serde_json::from_value::<CustomProvider>(json).unwrap(), row);
    }

    /// Custom key accounts follow one shape and only that shape is recognised.
    #[test]
    fn custom_key_account_shape() {
        assert_eq!(
            custom_key_account("api-libraxis-com"),
            "LLM_CUSTOM_API_LIBRAXIS_COM_API_KEY"
        );
        assert!(is_custom_key_account("LLM_CUSTOM_API_LIBRAXIS_COM_API_KEY"));
        assert!(!is_custom_key_account("LLM_CUSTOM__API_KEY"));
        assert!(!is_custom_key_account("LLM_OPENAI_API_KEY"));
        assert!(!is_custom_key_account("LLM_CUSTOM_X"));
    }

    /// The registry lists vendors first in picker order, then custom rows in
    /// settings order, and resolves both shapes with the contract's fields.
    #[test]
    fn registry_resolves_vendors_and_custom_rows() {
        let libraxis = CustomProvider::new(
            "api.libraxis.com",
            WireFamily::OpenAiResponses,
            "https://api.libraxis.com/v1/responses",
        )
        .unwrap();
        let row = libraxis;
        let registry = ProviderRegistry::new(vec![row.clone()]);
        let all = registry.all();
        assert_eq!(all.len(), 5);
        assert_eq!(
            all.iter()
                .map(|p| p.reference.as_string())
                .collect::<Vec<_>>(),
            [
                "libraxis-responses",
                "openai-responses",
                "xai-responses",
                "anthropic-messages",
                "custom:api-libraxis-com",
            ]
        );
        let libraxis = registry
            .resolve(&ProviderRef::Vendor(ProviderKind::LibraxisResponses))
            .unwrap();
        assert!(libraxis.key_required);
        assert_eq!(libraxis.oauth_vendor, None);
        assert!(libraxis.supports_vision("buddy"));
        let openai = registry.resolve(&ProviderRef::default()).unwrap();
        assert!(openai.key_required);
        assert_eq!(openai.oauth_vendor, Some(ProviderKind::OpenAiResponses));
        assert_eq!(openai.endpoint, "https://api.openai.com/v1/responses");
        assert_eq!(openai.key_account, "LLM_OPENAI_API_KEY");
        let custom = registry
            .resolve(&ProviderRef::Custom("api-libraxis-com".to_string()))
            .unwrap();
        assert_eq!(custom.display_name, "api.libraxis.com");
        assert_eq!(custom.wire, WireFamily::OpenAiResponses);
        assert_eq!(custom.endpoint, row.endpoint);
        assert_eq!(custom.key_account, row.key_account());
        assert!(!custom.key_required);
        assert_eq!(custom.oauth_vendor, None);
        assert!(custom.supports_vision("anything"));
        assert_eq!(
            registry.resolve(&ProviderRef::Custom("gone".to_string())),
            None
        );
        assert_eq!(registry.custom(), &[row]);
    }

    /// OpenAI policy stays permissive so the Responses request path is untouched.
    #[test]
    fn openai_policy_is_permissive_and_unchanged() {
        let p = capability_policy(ProviderKind::OpenAiResponses, "gpt-5.5");
        assert!(p.allow_sampling_params);
        assert!(p.previous_response_id);
        assert!(!p.refusal_stop_reason);
        assert!(!p.adaptive_thinking);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::NotApplicable);
        assert!(p.supports_vision);
        assert_eq!(
            capability_policy(ProviderKind::OpenAiResponses, "gpt-4.1"),
            p
        );
        // xAI shares the Responses wire and therefore the Responses policy.
        let xai = capability_policy(ProviderKind::XaiResponses, "grok-4.5");
        assert!(xai.previous_response_id);
        assert!(!xai.refusal_stop_reason);
    }

    /// Opus-4.8: no sampling params, hard-400 budget_tokens, strip temperature.
    #[test]
    fn opus_4_8_rejects_sampling_and_hard_400s_budget_tokens() {
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-opus-4-8");
        assert!(!p.allow_sampling_params);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Hard400);
        assert!(p.adaptive_thinking);
        assert!(p.effort);
        assert!(p.refusal_stop_reason);
        assert!(!p.previous_response_id);
        assert_eq!(p.sanitize_temperature(Some(0.7)), None);
    }

    /// Sonnet-4.6: temperature allowed; budget_tokens deprecated not hard-400.
    #[test]
    fn sonnet_4_6_tolerates_temperature_and_deprecates_budget_tokens() {
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-sonnet-4-6");
        assert!(p.allow_sampling_params);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Deprecated);
        assert_eq!(p.sanitize_temperature(Some(0.3)), Some(0.3));
    }

    /// Unrecognised Anthropic models inherit the strict (Opus) policy and stay
    /// vision-capable.
    #[test]
    fn unknown_anthropic_model_falls_back_to_strict_policy() {
        let p = capability_policy(ProviderKind::AnthropicMessages, "claude-future-9");
        assert!(!p.allow_sampling_params);
        assert_eq!(p.budget_tokens, BudgetTokensPolicy::Hard400);
        assert!(provider_supports_vision(
            ProviderKind::AnthropicMessages,
            "claude-future-9"
        ));
    }
}
