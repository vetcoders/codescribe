# ADR 2026-08-14 — Provider credential topology (keys and endpoints live together)

Status: ACCEPTED, NOT IMPLEMENTED (runtime re-verified 2026-08-22)
Owner: product contract; implementation remains a dedicated cut (W14 candidate)

The current Settings/config model still exposes lane-specific endpoint/key
slots and compatibility environment variables. No provider-registry-v2 type or
atomic provider reference owns all lanes yet. This ADR therefore constrains the
next implementation; it must not be cited as shipped behavior.

## Problem — the drift maker

Today credentials and endpoints live in two unrelated panels:

- `Settings → Providers` holds bare key slots (`LLM_API_KEY`,
  `LLM_FORMATTING_API_KEY`, `LLM_ASSISTIVE_API_KEY`, `STT_API_KEY`) plus the
  ChatGPT OAuth block, with no endpoint in sight;
- `Settings → Agent → LLM lanes` holds free-text endpoints and models per
  lane, with no credential in sight.

Nothing binds a key to the endpoint it authenticates. Measured consequences
(2026-08-12/14 field evidence, one morning):

1. **Chain poisoning** — Responses ids are scoped to the minting credential;
   a Keychain key swap left the stored chain id invisible to the new key
   (three `previous_response_not_found` failures, transcript delivered raw).
   Nothing invalidates conversation state on credential change
   (`reset_conversation*` has zero callers in config/keychain/account_auth).
2. **Two auth identities in one lane** — the OAuth-account-wins-over-key
   rule plus key-as-fallback lets one lane mint a chain under identity A and
   continue under identity B: `not_found` by construction.
3. **Slot asymmetry** — formatting/main resolve fixed slots while assistive
   resolves per-provider (`api_key_env_key`); onboarding wrote only the
   assistive slot and stale keys rotted silently in the others (silent 401,
   "no punctuation" class).
4. **Contradictory surface** — the assistive lane shows endpoint `api.x.ai`,
   a slot labeled "Assistive API key (OpenAI)", and a discovery banner from
   a third, dead xAI key. Three truths in one window.

## Decision — the operator's target shape (zero deviations)

One `Providers` surface with three tiers; **credential and endpoint are one
object, always**:

### Tier 1 — Vendors (endpoints PINNED, no URL field at all)

| Provider  | Credentials offered          | Endpoint                                |
| --------- | ---------------------------- | --------------------------------------- |
| OpenAI    | API key AND/OR ChatGPT OAuth | official, pinned, not shown as editable |
| xAI       | API key AND/OR xAI OAuth     | official, pinned                        |
| Anthropic | API key                      | official, pinned                        |

"Zero samowolki w urlach": a vendor row never exposes an endpoint editor.
If both credentials are present, the row states which one the runtime will
use (and the chain fingerprint — see Invariants).

### Tier 2 — Custom compatible providers

A list of user-defined rows, each an ATOMIC `{name, wire, endpoint, api_key}`:

- `wire = openai-responses | anthropic-messages`
- endpoint and key are entered, stored, tested and deleted TOGETHER;
- this is the only place a custom URL can exist (e.g. api.libraxis.cloud).

### Tier 3 — STT lanes

STT providers with explicit transports, same atomic credential+endpoint rule:

- `ws` (streaming sidecar/remote, W13-2B slot),
- `ndjson` (stt-jsonl-v1 class),
- `file` (multipart transcriptions).

### Lanes consume references, never raw URLs

`assistive / formatting / main / stt` each select a **provider reference**
(vendor or custom row) + model. The lane panel shows resolved truth only:
provider ref, credential KIND (key/account), model — no free endpoint field.

## Invariants (these close the measured failure classes)

- **I1**: a credential is never stored, tested, or deleted apart from its
  endpoint (atomic row).
- **I2**: conversation chain state is keyed by
  `(provider_ref, credential_fingerprint)`; any credential change resets the
  affected lanes' chains at write time (root fix for class 1/2; runtime
  self-heal `65e578e2` stays as backstop).
- **I3**: one resolution path for every lane (kills the fixed-slot vs
  per-provider split).
- **I4**: vendor endpoints are compile-time constants; only Tier-2 rows carry
  URLs.
- **I5**: the UI never shows a credential slot a lane cannot actually use.
- **I6**: OAuth persist is identity for that vendor row only. It must not
  probe another wire (official OpenAI Responses, Libraxis, a lane
  endpoint). Capability lives on the row/lane Test, not as a login gate.

## Migration sketch

1. Config model: provider registry v2 (`vendor` rows + `custom` rows + `stt`
   rows) with env/Keychain back-compat mapping from today's slots.
2. `lane_truth` resolves lane → provider ref → (endpoint, credential) in one
   step; delete the per-lane endpoint envs from the UI surface (env override
   stays for ops, marked as such).
3. Settings UI: single Providers screen with the three tiers; LLM lanes
   screen loses endpoint editors, keeps provider picker + model + resolved
   truth.
4. Chain fingerprint in `state::conversation` + reset hooks in
   keychain/account_auth writes.

## Out of scope here

Implementation. This ADR freezes the shape so no agent re-derives a
different one. Anti-pattern to reject on sight: "add one more key slot" or
"add an endpoint override field" — both re-open the drift maker.

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI
