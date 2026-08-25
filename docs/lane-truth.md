# Runtime Settings Snapshot — resolved LLM lanes

The runtime resolver is the single settings-loader pass that seals a
`RuntimeSettingsSnapshot`. There is no current `lane_truth_snapshot` resolver or
`core/llm/lane_truth.rs` module.

`SettingsLoader::load_runtime_snapshot` delegates to
`load_runtime_snapshot_with_keychain_population`, which reads the allowed
inputs, resolves all LLM lanes once, records provenance, computes a redacted
digest, and seals the immutable session snapshot. Consumers read
`RuntimeSettingsSnapshot::llm_lanes()`; they do not repeat settings, environment,
or credential resolution during a take.

## Snapshot shape

`RuntimeSettingsSnapshot` freezes:

- resolved `Config` values;
- the `UserSettings` intent read by the same loader pass;
- `RuntimeLlmLanes` for `main`, `formatting`, and `assistive`;
- the effective formatting policy;
- settings provenance and an integrity digest; and
- optional energy calibration.

Each `RuntimeLlmLane` contains its lane kind, provider, wire family, normalized
endpoint, model, resolved credential state, availability, and an optional
unavailable reason. `RuntimeLlmCredential` keeps the runtime key internally;
its `Debug` representation and snapshot digest expose presence only, never the
secret value.

## Lanes and credential accounts

| Lane | Runtime role | Credential account selected by the loader |
| --- | --- | --- |
| `main` | Shared/main LLM settings | `LLM_API_KEY` |
| `formatting` | Formatting lane | `LLM_FORMATTING_API_KEY` |
| `assistive` | Assistive and agent lane | The resolved provider's `api_key_env_key()` |

Provider identity and protocol are resolved before endpoint, model, credential,
and availability. The snapshot stores the result as typed `ProviderKind` and
`WireFamily`; downstream consumers do not infer protocol from URL or model text.

## Provider precedence

`resolve_runtime_llm_provider` applies lane-specific precedence:

- `assistive`: non-empty persisted `llm_assistive_provider`, then
  `LLM_ASSISTIVE_PROVIDER`, then the configured assistive-provider default;
- `formatting`: `LLM_FORMATTING_PROVIDER`, then the configured
  formatting-provider default;
- `main`: the configured main/assistive-provider default used by the loader.

Each candidate must parse as `ProviderKind`. Invalid or empty candidates fall
through to the next source; the final typed default is fail-safe.

## Model precedence

`resolve_runtime_llm_model` accepts only model ids owned by the resolved
provider.

For `formatting` and `assistive`, the lane-specific order is:

1. non-empty persisted lane model;
2. non-empty lane environment model;
3. for providers that own generic lane configuration, non-empty persisted
   shared `llm_model`;
4. for those same providers, non-empty shared `LLM_MODEL`;
5. the resolved provider's default for that lane.

`main` uses the shared persisted/environment sources before its default. A
model owned by a different provider is skipped rather than carried into the
sealed lane.

## Endpoint precedence

For `main` and providers that own generic lane configuration,
`resolve_runtime_llm_endpoint` uses:

1. non-empty persisted lane endpoint, where the lane has one;
2. non-empty lane endpoint environment value;
3. non-empty persisted shared `llm_endpoint`;
4. non-empty shared `LLM_ENDPOINT`;
5. the already-loaded `Config.llm_endpoint` value;
6. the default LLM endpoint.

For provider-owned configuration, the loader uses the provider's endpoint
environment key and then its default endpoint. In both cases the resolved
provider normalizes the final URL through `ProviderKind::normalize_endpoint`.

## Credential and availability truth

The sealed lane selects its credential account after provider resolution. The
loader records a non-empty resolved API key when present. Assistive account auth
is considered only for the OpenAI Responses wire family on an endpoint that
requires credentials and only when the provider OAuth token account is present.

A lane is available when at least one of these source-verified conditions holds:

- the resolved API key is present;
- the normalized endpoint does not require an API key; or
- the supported assistive account-auth condition is true.

Otherwise the snapshot seals an unavailable reason naming the lane, endpoint,
and credential account. That is runtime lane truth; a Settings probe answers a
different question and does not replace the sealed snapshot.

## Immutability and reset behavior

- One take uses one `RuntimeSettingsSnapshot`; consumers must not reread
  `settings.json` or process environment during the take.
- Settings changes create a new snapshot for a later session rather than
  mutating the in-flight value.
- `RecordingController` owns exactly one mutable settings handle,
  `RwLock<Arc<RuntimeSettingsSnapshot>>`. Hold scheduling, hold/toggle start,
  refresh and existing stop/finalize paths cross the same lifecycle lock.
- A pending delayed hold already owns its selected Arc, so refresh defers. A
  finished stale hold handle is consumed and does not block a later idle refresh.
- Empty candidates are normalized away by the resolver, allowing the next
  lower-precedence source to win.
- Never put real-looking keys in documentation. Persist credentials through the
  supported Settings/Keychain surface or inject them outside committed files.

## Canonical access

```text
settings.json
  → Config::load_runtime_snapshot()
  → resolve_runtime_llm_lanes + formatting_policy
  → RuntimeSettingsSnapshot::seal_loaded(...)
  → Arc / &RuntimeLlmLane / CsSettings::from_runtime_snapshot
```

Consumers of an active recording receive the controller's sealed Arc (or a
secret-free projection from it). Settings UI / tray / discovery boundaries may
mint a fresh snapshot; they must not reconstruct precedence via a second
`Config::load` + `UserSettings::load` + env merge. Explicit settings writes
affect only a later generation — idle refresh performs one Arc replacement;
an active or pending take keeps its immutable generation.

## Frozen unresolved formatter facts

The current frozen snapshot does not own formatting/assistive prompt content or
formatter retry/timing values. Provider execution still resolves prompt and
optional tuning files from `formatting_provider_system_prompt` inside its retry
loop. `RetryPolicy::module_defaults` also means the registered
`CODESCRIBE_AI_*` retry and timeout overlays do not control this formatter path.

Those facts are an explicit unresolved amendment boundary, not part of the
controller repair. Closing it requires an approved immutable owner for prompt
content/source evidence and effective retry/count/delay/timeout values. This cut
does not extend `RuntimeSettingsSnapshot`, add a second carrier, alter the
formatter, or remove the registered environment contract.

This document describes structure carried from executable cut `484095ce`, its
documentation successor `d57196ab`, and the C11 source cut. The actual C11 hash
lives only in its durable report. C11 did not exercise provider requests,
compiler gates, or any runtime credential path; those surfaces are
`NOT_ASSESSED`.
