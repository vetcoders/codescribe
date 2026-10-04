---
vendor: libraxis
canonical: libraxis-responses
module: core/llm/vendors/libraxis.rs
docs_read_on: 2026-09-07
plan: provider-registry-v2
cut: W1-T1-core-registry (added by contract amendment §B.3)
status: facts from the Founder's live probe + gateway source; no public docs page
---

# Libraxis gateway — wire specification

Libraxis is a first-class vendor (contract §B.3, Founder 2026-09-07) and the
last card in the picker until I1 promotes it on a live keyed witness. It speaks the OpenAI Responses protocol, so every
request builder reaches it through `WireFamily::OpenAiResponses`.

## Constants

| Constant                                               | Value                                                                           | Source                                                                 |
| ------------------------------------------------------ | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `CANONICAL`                                            | `libraxis-responses`                                                            | Codescribe registry spelling                                           |
| `ALIASES`                                              | `libraxis`, `lbrx`, `libraxis_responses`                                        | Codescribe registry spelling                                           |
| `DISPLAY_NAME`                                         | `Libraxis`                                                                      | picker label                                                           |
| `DOCS_URL`                                             | https://github.com/LibraxisAI/lbrx-services (branch `feat/vista-brain-revival`) | gateway source; no public REST docs                                    |
| `ENDPOINT`                                             | `https://api.libraxis.com/v1/responses`                                         | live probe 2026-09-07                                                  |
| `MODELS_ENDPOINT`                                      | `https://api.libraxis.com/v1/models`                                            | verified live 2026-09-07 with a key (HTTP 200); 401 without            |
| `API_KEY_ACCOUNT`                                      | `LLM_LIBRAXIS_API_KEY`                                                          | Codescribe Keychain bundle account                                     |
| `AUTH_HEADER` / `AUTH_VALUE_PREFIX`                    | `authorization` / `Bearer `                                                     | OpenAI-style bearer                                                    |
| `EXTRA_HEADERS`                                        | none                                                                            | —                                                                      |
| `DEFAULT_FORMATTING_MODEL` / `DEFAULT_ASSISTIVE_MODEL` | `buddy` / `buddy`                                                               | gateway profiles: `buddy`, `programmer`, `soap`, `chat`, `suggestions` |
| `HOSTS`                                                | `api.libraxis.com`, `api.libraxis.cloud`                                        | both hosts migrate to this vendor row                                  |

`key_required = true` (401 without a key). `oauth_vendor = None`: the gateway
has a session token, but no desktop sign-in flow. Vision follows the Responses
policy (permissive; the gateway has a VLM path).

## Verified live on 2026-09-07 (keyed probe by the plan session)

- `GET /v1/models` → HTTP 200, `{"object":"list","data":[{"id","object":"model", "owned_by":"libraxis","is_alias":true,"modalities":[…],"capability_matrix":{"image":true,…}}]}`,
  eight aliases: `master`, `soap`, `ai-suggestions`, `chat`, `programmer`,
  `svetliq`, `gpt-oss-20b`, `buddy`. Raw body pinned as
  `core/llm/vendors/fixtures/libraxis_models_live_2026-09-07.json`; the parser
  reads `data[].id` only.
- `POST /v1/responses` → HTTP 503 `{"error":{"code":"no_eligible_provider",…}}`
  on that day: gateway routing, not auth. The liveness probe surfaces
  `error.code` so Test connection distinguishes the two.

## Still unverified (`[!]`)

- The one-token `liveness_probe_body` has not returned a 200 yet (503 above).

## Migration

Legacy `speech.llm_endpoint` / `speech.assistive.llm_endpoint` pointing at either
host resolve to `libraxis-responses` (vendor), not to a Custom row. The div0
fixture in `core/config/llm_migration.rs` pins this: both lanes on the vendor,
model `buddy`, zero custom rows, three key moves into `LLM_LIBRAXIS_API_KEY`.
