# Lane Truth — LLM configuration resolver

`lane_truth` is the runtime resolver for LLM provider identity, endpoints,
models, and credential presence. The documentation below describes the current
code path, not a deprecated `.env` sample.

## Lanes

| Lane                | Runtime use                                   | Provider                                   | Credential account                                                         |
| ------------------- | --------------------------------------------- | ------------------------------------------ | -------------------------------------------------------------------------- |
| `main`              | shared fallback for OpenAI Responses settings | `openai-responses`                         | `LLM_API_KEY`                                                              |
| `formatting`        | cleanup / formatting pass                     | `openai-responses` or `anthropic-messages` | `LLM_FORMATTING_API_KEY` for OpenAI, `LLM_ANTHROPIC_API_KEY` for Anthropic |
| `assistive` / agent | assistive chat and the app agent provider     | `openai-responses` or `anthropic-messages` | `LLM_ASSISTIVE_API_KEY` for OpenAI, `LLM_ANTHROPIC_API_KEY` for Anthropic  |

Provider ids are literal: `openai-responses` and `anthropic-messages`.
Friendly aliases may parse in code, but persisted docs and examples should use
the canonical kebab-case ids.

## Precedence

The OpenAI endpoint resolver for `formatting` and `assistive` uses this order:

1. fresh lane setting from `settings.json`
2. lane env (`LLM_FORMATTING_ENDPOINT` or `LLM_ASSISTIVE_ENDPOINT`)
3. fresh shared setting (`llm_endpoint`)
4. shared env (`LLM_ENDPOINT`)
5. loaded config value
6. default `https://api.openai.com/v1/responses`

OpenAI model resolution follows the same lane-before-shared shape, except it has
no loaded-config fallback: fresh lane setting → lane env → fresh shared setting
→ shared env → lane default. Defaults are `gpt-4.1` for formatting and
`gpt-5.5` for assistive. Claude model ids are ignored by the OpenAI model path.

Provider resolution is lane-specific. Formatting currently reads
`LLM_FORMATTING_PROVIDER` from env. Assistive reads the fresh persisted
`llm_assistive_provider` first, then falls back to `LLM_ASSISTIVE_PROVIDER`.
Invalid or missing provider values fall back to `openai-responses`.

Anthropic endpoint resolution is provider-owned: `anthropic-messages` uses
`https://api.anthropic.com/v1/messages`. Its lane defaults are
`claude-sonnet-4-6` for formatting and `claude-opus-4-8` for assistive, unless
a Claude model is configured through the same lane/shared hierarchy.

## Credential and reset contract

- Snapshots are secret-free: they expose `key_present`, `key_account`,
  `account_auth`, availability, and a human-readable unavailable reason, never
  the secret value.
- Explicit non-empty process env wins over Keychain for credential lookup;
  empty or missing env falls back to Keychain.
- Reset means unset/remove the value. Empty persisted settings are normalized to
  `None`, so the next lower-precedence layer is used.
- Do not put real-looking keys in docs. Store secrets through Settings / macOS
  Keychain or use non-secret placeholders in examples.

## Endpoint normalization

- OpenAI-compatible endpoints are normalized to `/v1/responses`. Examples:
  `https://api.openai.com` and `https://api.openai.com/v1` both resolve to
  `https://api.openai.com/v1/responses`.
- Anthropic Messages resolves to `/v1/messages`.
- Key-optional endpoints are allowed only for non-official OpenAI-compatible
  hosts. The guidance endpoint `https://api.libraxis.cloud/v1` is suggested for
  setup text, but runtime never silently reroutes traffic there.

## Availability and diagnostics

Assistive / agent availability is true when any of these is true:

- an API key exists for the resolved provider account,
- an OpenAI Responses lane has ChatGPT account auth on the official OpenAI host,
- the endpoint is an allowed key-optional non-official host.

Official OpenAI and Anthropic endpoints require credentials. ChatGPT account auth
is only used for official OpenAI Responses and is intentionally not sent to
key-optional endpoints.

The Settings key probe and the agent gate answer different questions. The probe
checks whether a stored key can perform a minimal provider request and reports
states such as invalid key, no credits, network error, or unsupported. The agent
gate asks whether the resolved lane is available at runtime after endpoint,
provider, key, Keychain, and account-auth resolution.

## Testable examples

OpenAI formatting with shared fallback:

```env
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1
LLM_FORMATTING_MODEL=gpt-4.1
# Store LLM_FORMATTING_API_KEY in Keychain or set it outside committed files.
```

Assistive / agent through a key-optional local-compatible endpoint:

```env
LLM_ASSISTIVE_ENDPOINT=http://127.0.0.1:11434/v1
LLM_ASSISTIVE_MODEL=local-assistive-model
# No API key is required for this non-official local endpoint.
```

Anthropic assistive:

```env
LLM_ASSISTIVE_PROVIDER=anthropic-messages
LLM_ASSISTIVE_MODEL=claude-opus-4-8
# Store LLM_ANTHROPIC_API_KEY in Keychain or set it outside committed files.
```

To reset a lane endpoint or model, remove that env line or clear the field in
Settings. Do not replace it with a fake key or an empty quoted secret in docs.
