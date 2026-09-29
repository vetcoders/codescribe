---
vendor: anthropic
canonical: anthropic-messages
module: core/llm/vendors/anthropic.rs
docs_read_on: 2026-09-07
plan: provider-registry-v2
cut: W1-T1-core-registry
status: read-only reference; Findings are for I1, not fixes in this cut
---

# Anthropic Messages API — wire specification

Source of truth for every constant in `core/llm/vendors/anthropic.rs`. Read on
2026-09-07 from the official pages listed per row. When this page and the
docs disagree, the docs win.

## Constants

| Constant                   | Value                                            | Source                                                                                          |
| -------------------------- | ------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `CANONICAL`                | `anthropic-messages`                             | Codescribe registry spelling (settings.json, env)                                               |
| `ALIASES`                  | `anthropic`, `anthropic_messages`                | Codescribe registry spelling                                                                    |
| `DISPLAY_NAME`             | `Anthropic (Messages)`                           | Codescribe picker label                                                                         |
| `DOCS_URL`                 | https://platform.claude.com/docs/en/api/messages | —                                                                                               |
| `ENDPOINT`                 | `https://api.anthropic.com/v1/messages`          | https://platform.claude.com/docs/en/api/messages — "POST https://api.anthropic.com/v1/messages" |
| `MODELS_ENDPOINT`          | `https://api.anthropic.com/v1/models`            | https://platform.claude.com/docs/en/api/models-list — "GET /v1/models"                          |
| `API_KEY_ACCOUNT`          | `LLM_ANTHROPIC_API_KEY`                          | Codescribe Keychain bundle account                                                              |
| `AUTH_HEADER`              | `x-api-key`                                      | https://platform.claude.com/docs/en/api/messages — Headers: `x-api-key` (required)              |
| `AUTH_VALUE_PREFIX`        | `` (empty)                                       | same page: the raw key is the header value                                                      |
| `EXTRA_HEADERS`            | `anthropic-version: 2023-06-01`                  | https://platform.claude.com/docs/en/api/versioning — "current version: 2023-06-01"              |
| `DEFAULT_FORMATTING_MODEL` | `claude-sonnet-5`                                | https://platform.claude.com/docs/en/about-claude/models/overview — current lineup on 2026-09-07 |
| `DEFAULT_ASSISTIVE_MODEL`  | `claude-opus-5`                                  | same page                                                                                       |

The endpoint is pinned in code. There is no env or settings override for a
vendor endpoint; a different host is a Custom provider on the `messages` wire.

## `POST /v1/messages` — minimal request

Required body fields per https://platform.claude.com/docs/en/api/messages:
`model`, `max_tokens`, `messages`. Required headers: `x-api-key`,
`anthropic-version`, `content-type: application/json`.

```json
{
  "model": "claude-sonnet-5",
  "max_tokens": 1,
  "messages": [{ "role": "user", "content": "ping" }]
}
```

`liveness_probe_body(model)` emits exactly that shape. `content` accepts either
a string or an array of content blocks; both forms are documented on the same
page.

## `GET /v1/models` — response shape

Per https://platform.claude.com/docs/en/api/models-list:

- Query: `after_id`, `before_id`, `limit` (default 20, max 1000).
- Response: `{ "data": [ { "id", "display_name", "created_at", "type": "model" } ], "first_id", "has_more", "last_id" }`.
- Pagination: "`last_id` — Last ID in the data list. Can be used as the
  `after_id` for the next page."
- Auth: the page's curl example sends `X-Api-Key` and `anthropic-version: 2023-06-01`.

`models_from_response` reads one page into `(id, display_name)` pairs. The
caller owns pagination.

## Authentication

- API key: `x-api-key: <key>`. Anthropic does not accept `Authorization: Bearer`
  for API keys (https://platform.claude.com/docs/en/api/messages, Headers).
- OAuth (Sign in with Claude): owned by `core/llm/account_auth` via the
  `ANTHROPIC_OAUTH` row; token account `LLM_ANTHROPIC_ACCOUNT_TOKENS`. Out of
  scope for this cut; the vendor row only marks `oauth_vendor = Some(AnthropicMessages)`
  because that row exists.

## Current model list (read 2026-09-07)

From https://platform.claude.com/docs/en/about-claude/models/overview:

- Current: `claude-fable-5-1`, `claude-opus-5`, `claude-sonnet-5`,
  `claude-haiku-4-5-20251001` (alias `claude-haiku-4-5`).
- Still served (legacy): `claude-fable-5`, `claude-opus-4-8`, `claude-opus-4-7`,
  `claude-opus-4-6`, `claude-opus-4-5`, `claude-sonnet-4-6`, `claude-sonnet-4-5`.
- Extended thinking with a manual `budget_tokens` is deprecated on the 4.6
  generation and not accepted on later models.

Live truth is `GET /v1/models`; this list only dates the defaults.

## Findings — drift between the docs and the current wire code

Read-only. Nothing below was changed in W1-T1; the owners are I1 or a follow-up
cut.

1. **Four copies of the version header.** `core/llm/ai_formatting.rs:115`,
   `app/agent/anthropic_provider.rs:47`, `core/llm/key_liveness.rs:22` and
   `core/llm/model_discovery.rs` each declare `ANTHROPIC_VERSION = "2023-06-01"`.
   All four agree with the versioning page today. `vendors::anthropic::EXTRA_HEADERS`
   is the single source from now on; I1 should rewire the four call sites.
2. **Stale default models.** `core/llm/provider.rs` (baseline 364bb01ec,
   `ANTHROPIC_IDENTITY`) seeds `claude-sonnet-4-6` / `claude-opus-4-8`. The models
   overview lists `claude-sonnet-5` / `claude-opus-5` as current. The registry row
   now reads the vendor module, so the seeds move with the docs.
3. **Capability policy keyed on a substring.** `core/llm/provider.rs::anthropic_policy_for_model`
   returns `BudgetTokensPolicy::Deprecated` and `allow_sampling_params = true` for
   any id containing `sonnet`. The overview states a manual `budget_tokens` is
   "not accepted on later models", so `claude-sonnet-5` should map to the strict
   branch for `budget_tokens`. Whether Sonnet 5 accepts `temperature` was not on
   the pages read on 2026-09-07 (`[!]` unverified). Nothing in Codescribe sends
   `budget_tokens`, so this is a policy-label drift, not a live 400 today.
4. **`/v1/models` derived from the lane endpoint.** `core/llm/model_discovery.rs::fetch_anthropic_models`
   (baseline ~line 428) builds the models URL by rewriting the lane's
   `/v1/messages` path through `provider_models_endpoint`. For the vendor that is
   equivalent to `MODELS_ENDPOINT`; for a Custom provider on the `messages` wire it
   is the only workable option, so the rewrite stays and the vendor constant is
   documentation of the expected result.
5. **Pagination matches the docs.** The same function follows `has_more` with
   `after_id = last_id`, exactly as the models-list page describes, and falls back
   to the last `data[].id` when `last_id` is absent. No drift; noted so I1 does not
   "fix" it.
6. **Liveness probe body uses content blocks.** `core/llm/key_liveness.rs::probe_anthropic_key`
   sends `content: [{"type":"text","text":"ping"}]` with `max_tokens: 1`. The docs
   accept both the string and block forms; `liveness_probe_body` uses the string
   form because it is the documented minimal example. Both are valid.
7. **Adaptive thinking request shape.** `app/agent/anthropic_provider.rs::build_request_body`
   (~line 294) sends `"thinking": {"type": "adaptive"}`. The `thinking` object
   was not on the four pages read on 2026-09-07, so its current schema is
   `[!]` unverified here. It is not part of this cut's contract.
8. **Formatting path is docs-clean.** `core/llm/ai_formatting.rs::call_anthropic_messages_resolved`
   (~line 1543) sends `anthropic-version`, `content-type: application/json`,
   `x-api-key` only when a key exists, and the three required body fields. No
   drift.

## Pages read on 2026-09-07

- https://platform.claude.com/docs/en/api/messages
- https://platform.claude.com/docs/en/api/models-list
- https://platform.claude.com/docs/en/about-claude/models/overview
- https://platform.claude.com/docs/en/api/versioning

Not read (context7 resolves only SDK packages for "anthropic"; the REST pages
were fetched directly): the extended-thinking page, the OAuth page.
