---
plan_id: provider-registry-v2
run_id: impl-260907-184236-24292
session_id: 01a07cc0-1a15-7942-a3ae-63d615cc5aaa
role: design-doc
agent: codex
date: 2026-09-07
project: vetcoders/codescribe
---

# OpenAI provider specification

## Pending evidence first

[?] Compilation, module tests, live key probe and registry integration belong to I1.
The one-token probe implements atlas B.0; no live call establishes that every model
accepts this cap. A reasoning model can consume its budget without visible text.
This file documents a vendor specification, not installed runtime behavior.

## Docs przeczytane

All retrieved 2026-09-07, before code, through official OpenAI developer-docs MCP.
Read skill: `/Users/maciejgad/.claude/skills/openai-docs/SKILL.md`.
No callable OpenAI MCP tool was initially exposed. `codex mcp add openaiDeveloperDocs --url https://developers.openai.com/mcp` succeeded.
Direct HTTP MCP `tools/list`, `search_openai_docs`, `fetch_openai_doc` and
`get_openapi_spec` succeeded without restarting this headless worker.

- [Responses — Create](https://developers.openai.com/api/reference/resources/responses/methods/create) — 2026-09-07.
- [Models — List](https://developers.openai.com/api/reference/resources/models/methods/list) — 2026-09-07.
- [API Overview / Authentication](https://developers.openai.com/api/reference/overview) — 2026-09-07.
- [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses) — 2026-09-07.
- [GPT-4.1](https://developers.openai.com/api/docs/models/gpt-4.1) — 2026-09-07.
- [GPT-5.2](https://developers.openai.com/api/docs/models/gpt-5.2) — 2026-09-07.
- [GPT-5.5](https://developers.openai.com/api/docs/models/gpt-5.5) — 2026-09-07.
- [Models catalog](https://developers.openai.com/api/docs/models) — 2026-09-07.
- [Authentication (Codex / desktop)](https://developers.openai.com/codex/auth) — 2026-09-07.

The generated create/list reference fetches return stubs. Read their full endpoint
OpenAPI examples using `get_openapi_spec` for `https://api.openai.com/v1/responses`
and `https://api.openai.com/v1/models`. Schema references in that response are not
expanded; MCP search returned the create parameter descriptions (including the
output-token bound). Fetch with anchor still returned the stub; fallback web fetch
failed on page size. Thus no documented numerical minimum was established.

## Stałe

Local identity names and the Keychain account come from baseline/atlas D3; vendor
URLs below establish the associated wire, not those Codescribe-specific strings.
Model choices retain baseline seeds because both remain documented; this is not
an allowlist or a claim about this user's live model entitlements.

| Constant                   | Value                                                                              | Source / provenance                                                                                                   |
| -------------------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `CANONICAL`                | `"openai-responses"`                                                               | [Docs](https://developers.openai.com/api/reference/resources/responses/methods/create); Codescribe policy             |
| `ALIASES`                  | `&["openai", "openai_responses"]`                                                  | [Docs](https://developers.openai.com/api/reference/resources/responses/methods/create); Codescribe policy             |
| `DISPLAY_NAME`             | `"OpenAI (Responses)"`                                                             | [Docs](https://developers.openai.com/api/reference/resources/responses/methods/create); Codescribe policy             |
| `DOCS_URL`                 | `"https://developers.openai.com/api/reference/resources/responses/methods/create"` | [Docs](https://developers.openai.com/api/reference/resources/responses/methods/create); vendor wire / documented seed |
| `ENDPOINT`                 | `"https://api.openai.com/v1/responses"`                                            | [Docs](https://developers.openai.com/api/reference/resources/responses/methods/create); vendor wire / documented seed |
| `MODELS_ENDPOINT`          | `"https://api.openai.com/v1/models"`                                               | [Docs](https://developers.openai.com/api/reference/resources/models/methods/list); vendor wire / documented seed      |
| `API_KEY_ACCOUNT`          | `"LLM_OPENAI_API_KEY"`                                                             | [Docs](https://developers.openai.com/api/reference/overview#authentication); Codescribe policy                        |
| `AUTH_HEADER`              | `"authorization"`                                                                  | [Docs](https://developers.openai.com/api/reference/overview#authentication); vendor wire / documented seed            |
| `AUTH_VALUE_PREFIX`        | `"Bearer "`                                                                        | [Docs](https://developers.openai.com/api/reference/overview#authentication); vendor wire / documented seed            |
| `EXTRA_HEADERS`            | `&[]`                                                                              | [Docs](https://developers.openai.com/api/reference/overview#authentication); vendor wire / documented seed            |
| `DEFAULT_FORMATTING_MODEL` | `"gpt-4.1"`                                                                        | [Docs](https://developers.openai.com/api/docs/models/gpt-4.1); vendor wire / documented seed                          |
| `DEFAULT_ASSISTIVE_MODEL`  | `"gpt-5.5"`                                                                        | [Docs](https://developers.openai.com/api/docs/models/gpt-5.5); vendor wire / documented seed                          |

## Models response shape

`GET /v1/models` returns `object: "list"` and `data` model objects containing `id`,
`object`, `created`, `owned_by`; the current official example also has
`shutdown_date`. There is no documented display-name field. The parser returns
`(id, None)` in response order without model-prefix vetos or shutdown filtering.
Missing/non-array `data` produces an empty vector; malformed ids are skipped.
Those failure semantics are local policy required by the non-Result interface.

Fixture source: Models List OpenAPI `paths./models.get.x-oaiMeta.examples.response`.
All three official placeholder records are preserved, including shutdown metadata.
The upstream example ends its final array member with an invalid JSON trailing
comma. Only that comma was removed to make the copied fixture parseable; no data
was invented. The fixture ids are examples, not current model ids.

## Authentication headers

API keys use `authorization: Bearer <key>`. JSON transport supplies
`Content-Type: application/json`. No mandatory additional vendor headers are
specified; optional organization/project selection is not a static extra header.
The internal `LLM_OPENAI_API_KEY` account is atlas D3 policy, not the vendor's
`OPENAI_API_KEY` environment convention. See API Overview above.

## Models checked

GPT-4.1: `gpt-4.1`, snapshot `gpt-4.1-2025-04-14`.
GPT-5.x pages checked: `gpt-5.2` / `gpt-5.2-2025-12-11` and
`gpt-5.5` / `gpt-5.5-2026-04-23`. All list Responses support.
Keep baseline formatting `gpt-4.1` and assistive `gpt-5.5`.
The catalog also contains newer families; the contract does not request a model
upgrade merely because a newer family exists. Runtime discovery remains authority.

## OAuth

Official Authentication docs describe ChatGPT sign-in for OpenAI desktop/CLI/IDE,
browser callback and headless device-code options, including localhost:1455.
They do not establish a public third-party desktop registration contract for
Codescribe's reused client id, `codescribe_account_flow`, or ChatGPT token access
to the pinned public API endpoint. For those details: **brak w docs** read here.
No OAuth implementation was changed or certified.

## Findings

1. **P2 — undocumented duplicate authentication header.**
   `core/llm/key_liveness.rs:310`, `core/llm/ai_formatting.rs:358,1706,1773`,
   `app/agent/openai_provider.rs:224` add `x-api-key` or select
   `BearerAndApiKey` on the OpenAI API-key path. API Overview says
   “Provide API credentials with HTTP Bearer authentication.” The extra header
   is not part of the documented OpenAI key contract. No live evidence here says
   it is rejected. I1/follow-up should select headers per vendor.
2. **P2 — account-auth wire remains unproven.**
   `core/llm/account_auth/mod.rs:179,189` uses the Codex public client id and
   `codescribe_account_flow`; `app/agent/openai_provider.rs:195,221` obtains an
   account token and chooses Bearer. Official Authentication says “For general
   OpenAI API calls, continue to use Platform API keys.” Its documented Codex
   sign-in is not proof that Codescribe's flow authorizes `/v1/responses`.
   I1 must retain explicit unknown status until appropriate runtime evidence.
3. **Probe budget limitation, not a proven wire bug.**
   `core/llm/key_liveness.rs:302` already uses `max_output_tokens: 1`.
   The create docs count “visible output tokens and reasoning tokens” together.
   The new body preserves the requested one-token policy, uses documented string
   input, and does not promise text or universal acceptance. I1 must separate
   model/budget rejection from invalid credentials in a live probe.
4. **No discrepancy found in reviewed protocol selection and discovery shape.**
   `request_thread_title` (321), `request_responses_thread_title` (332),
   `call_provider_once` (1475), `call_llm_endpoint` (1642),
   `call_llm_endpoint_streaming` (1759) in `core/llm/ai_formatting.rs` route
   Responses, send `input`, and set `stream` consistently with the streaming guide.
   `OpenAiProvider::stream` sends `stream: true`, `max_output_tokens` and delegates
   SSE to the shared manager. `fetch_openai_models` at
   `core/llm/model_discovery.rs:394` reads `data[].id` and uses Bearer.
   Full SSE event handling was not audited in this bounded vendor cut.
   `fetch_anthropic_models` (428), `probe_anthropic_key` (319), and
   `app/agent/anthropic_provider.rs::stream` (135) were read only as dispatch
   boundaries; this OpenAI evidence does not certify Anthropic's wire.

## Integration boundary

No behavior in provider.rs, config, OAuth or app was edited. I1 replaces registry
row literals with this module and reconciles the three vendor declarations.
No additional dependency, parallel request path, model veto or endpoint override
was introduced. Auth/fixture limitations above do not change frozen B.0 signatures.
