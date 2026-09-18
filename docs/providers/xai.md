---
plan_id: provider-registry-v2
run_id: impl-260907-184236-68672
session_id: 36acb38d-3e95-4838-a804-05151a3f58d9
role: design-doc
agent: grok
date: 2026-09-07
project: vetcoders/codescribe
---

# xAI (Grok) provider pin

Read 2026-09-07 from official docs.x.ai. Codescribe account is `LLM_XAI_API_KEY`
(Keychain), not the skill's `XAI_API_KEY`.

## Stałe

| Nazwa                    | Wartość                                           | URL                                                                                                     |
| ------------------------ | ------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| CANONICAL                | `xai-responses`                                   | Codescribe id (not an xAI slug). Host: https://docs.x.ai/developers/rest-api-reference                  |
| ALIASES                  | `xai`, `grok`, `xai_responses`                    | Existing `ProviderIdentity` spellings; docs do not list them                                            |
| DISPLAY_NAME             | `xAI (Grok)`                                      | https://docs.x.ai/overview                                                                              |
| DOCS_URL                 | `https://docs.x.ai/developers/rest-api-reference` | REST overview                                                                                           |
| ENDPOINT                 | `https://api.x.ai/v1/responses`                   | https://docs.x.ai/developers/rest-api-reference/inference/responses                                     |
| MODELS_ENDPOINT          | `https://api.x.ai/v1/models`                      | https://docs.x.ai/developers/rest-api-reference/inference/models                                        |
| API_KEY_ACCOUNT          | `LLM_XAI_API_KEY`                                 | Codescribe Keychain. Vendor header uses `$XAI_API_KEY`: https://docs.x.ai/developers/rest-api-reference |
| AUTH_HEADER              | `authorization`                                   | `Authorization: Bearer <your xAI API key>` — https://docs.x.ai/developers/rest-api-reference            |
| AUTH_VALUE_PREFIX        | `Bearer `                                         | same                                                                                                    |
| EXTRA_HEADERS            | `[]`                                              | Inference REST documents Bearer only                                                                    |
| DEFAULT_FORMATTING_MODEL | `grok-4.6`                                        | https://docs.x.ai/developers/grok-4-6 (seed 2026-09-07)                                                 |
| DEFAULT_ASSISTIVE_MODEL  | `grok-4.6`                                        | same                                                                                                    |

## Auth headers

Every inference request: `Authorization: Bearer <key>` plus `Content-Type: application/json`.
Base: `https://api.x.ai`. Chat Completions (`/v1/chat/completions`) is documented as
deprecated; Responses is the primary interface
(https://docs.x.ai/developers/model-capabilities/text/comparison).

mTLS (`https://mtls.api.x.ai`) is an extra layer for teams that enroll a CA; it does
not replace Bearer (https://docs.x.ai/developers/advanced-api-usage/mtls).

## Kształt `/v1/models`

`GET /v1/models` returns `{ "object": "list", "data": [ { "id", "object": "model", "owned_by", "created", pricing… } ] }`. There is **no** `display_name` field.
Fixture copied into `models_from_response` tests:
https://docs.x.ai/developers/rest-api-reference/inference/models.md

`GET /v1/language-models` returns `{ "models": [ { "id", "aliases", modalities, fingerprint, version, pricing… } ] }`. Still no display-name field; extra vs
`/v1/models` is modalities / fingerprint / aliases.

## OAuth

Public inference docs document API-key Bearer only. They do **not** document a
desktop OAuth / device-code / PKCE flow for third-party apps. Codescribe already
ships `XAI_OAUTH` (`https://auth.x.ai`, Grok CLI public client id, device-code
login) in `core/llm/account_auth/mod.rs` — that is in-tree product, not a
docs.x.ai contract. I1/follow-up: do not treat OAuth as vendor-REST truth.

## Modele (seed 2026-09-07)

Live list = `GET /v1/models`. Docs seed, not runtime truth:

- `grok-4.6` — flagship on homepage + Responses examples
  (https://docs.x.ai/developers/grok-4-6). Still listed: `grok-4.5`
  (https://docs.x.ai/developers/model-capabilities/text/reasoning,
  https://docs.x.ai/developers/rate-limits).
- Skill `~/.grok/skills/xai-api/SKILL.md` still names `grok-4.5` and shows
  `POST /v1/chat/completions` — stale vs docs.x.ai today.

## Findings

Wire vs docs. Repair is I1 / follow-up. This lane did not edit these files.

1. **Default model is `grok-4.5` in the registry row; docs seed `grok-4.6`.**
   `core/llm/provider.rs:109` `const DEFAULT_XAI_MODEL: &str = "grok-4.5";`
   Homepage curl (2026-09-07) uses `"model": "grok-4.6"` on `POST https://api.x.ai/v1/responses`
   (https://docs.x.ai/overview). `grok-4.5` is not retired. Founder walk-around
   wants Formatting on xAI to keep `grok-4.5` — I1 must choose; this module
   seeds docs (`grok-4.6`).

2. **Dual `x-api-key` header on the Responses path — not in xAI docs.**
   `core/llm/key_liveness.rs:307-309` (`probe_responses_key`): `.bearer_auth(api_key).header("x-api-key", api_key)`.
   Same on `request_responses_thread_title` `ai_formatting.rs:357-358` and
   `ai_formatting.rs:19` module comment ("Bearer + x-api-key"). xAI REST:
   `Authorization: Bearer <your xAI API key>` only
   (https://docs.x.ai/developers/rest-api-reference). `x-api-key` is Anthropic's
   header. Extra header is undocumented for `api.x.ai`; may be ignored or may
   confuse a proxy.

3. **Liveness body is richer than the documented minimum.**
   `probe_responses_key` `key_liveness.rs:296-304` sends `input` as an
   `input_text` array plus `"stream": false`. Docs example is
   `{ "model": "grok-4.6", "input": "What is the meaning of life?" }`
   (https://docs.x.ai/developers/rest-api-reference/inference/responses).
   Array form is also legal (`input`: string | array). `max_output_tokens: 1`
   matches the Responses name; do not send `max_tokens`.

4. **Discovery copies `id` as `display_name`; docs have no display name.**
   `fetch_openai_models` `model_discovery.rs:416-421`:
   `display_name: model.id.clone()`. `/v1/models` example has `id` only
   (https://docs.x.ai/developers/rest-api-reference/inference/models.md).
   `/v1/language-models` adds `aliases`, still no display-name. xAI is routed
   through this OpenAI parser because `WireFamily::OpenAiResponses`.

5. **`LLM_XAI_ENDPOINT` still exists as an override. D1 pins the URL.**
   `provider.rs:153` `endpoint_env: "LLM_XAI_ENDPOINT"`.
   `docs/ENV_REGISTRY.toml:896-901` describes it as a proxy override.
   Atlas §E deletes `LLM_XAI_ENDPOINT` after I1. This module's `ENDPOINT` is
   the nail.

6. **Skill vs REST: Chat Completions example.**
   `~/.grok/skills/xai-api/SKILL.md` fetch example is
   `https://api.x.ai/v1/chat/completions`. Docs: Responses is primary; Chat
   Completions is the legacy column
   (https://docs.x.ai/developers/model-capabilities/text/comparison).
   Codescribe Agent path (`app/agent/openai_provider.rs:1`) already stays on
   `POST /v1/responses` SSE — that matches docs. Do not degrade.

7. **OAuth: in-tree, not in public REST docs.**
   `account_auth/mod.rs:222-247` `XAI_OAUTH` (issuer `https://auth.x.ai`,
   device-code, Grok CLI client id `b1a00492-…`). Searched docs.x.ai for
   desktop OAuth / device-code / PKCE for third-party apps — no REST page.
   Note only.

Reviewed (read-only) and no additional wire/docs clash beyond the list above:
`call_provider_once` `ai_formatting.rs:1475` (xAI rides `WireFamily::OpenAiResponses`),
`fetch_anthropic_models` / `probe_anthropic_key` (Anthropic-only).
