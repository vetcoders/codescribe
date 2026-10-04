# W-0: decision history and runtime inventory

Source baseline: `release/0.15.2-dragon @ d2ea631a10f1c13f3c5be3773eb25c1d1f62f9a8`.
This inventory describes source corridors, not installed application acceptance.

## Trail contract

`core/pipeline/trail.rs` writes `codescribe.decision-trail.v1` JSONL beside the
slot sidecar. `SlotReceiptSink` owns the trail lifetime in the existing
`apple_stream_worker`, including that worker's cloud lane. Tests use temporary
roots or the existing test isolation. Ordinary ledger tests perform no IO.

There is one decision hook, `AcousticLedger::record_layer_decision`. Its existing
receipt ID remains unchanged; `producer` distinguishes Lexicon and Formatter
from their shared `retained_text` layer. The hook captures candidate text/tokens,
observation identity, verdict, predecessor and resulting word slots. The
`admit_with_slots` corridor additionally captures the exact previous slots and
reconciled admission input. Qualification events retain the measured evidence
and the actual calibration. Frontier events record schedule/observer/return
calls; projection events retain actual reducer revision transitions and receipt
references. None of these observations returns transcript authority.

Recording every decision instead of waiting for a seal matters: a refused or
unqualified observation may never seal, and diagnostics must still show it.
Persistence closure is the STT session sink's drop. This is not a delivery or
whole-document terminal receipt: post-Stop formatter work can outlive that sink
and therefore escape this registration. JSON encoding, directory creation,
file open/write and flush run on a named IO thread. The existing slot sidecar's
IO is unchanged. The trail uses a 64-record bounded queue and nonblocking
`try_send`; overflow/disconnection increments `dropped_records`. Sink drop
closes the queue and joins on the STT worker. The file is exclusively created
with mode 0600, never overwritten. A worker write error or a crash leaves no
successful end receipt. Trace labels a missing end or overflow incomplete.

**Coverage limits:** `evidence_complete` is currently always false. Frontier
call timestamps are measured, but job request/generation-bound scheduling,
return and authority-transfer instrumentation is not complete. Decision-level
`scheduled_ns`, `returned_ns`, `transferred_ns` and reducer revision fields
remain null; real revision transitions are separate projection records. A
frontier return is not evidence of N2 handoff. Late-Apple scheduling, derived
occurrence qualification, seals, full document revision inputs and other
admission corridors still need replay records. The registration is the existing
Apple/cloud streaming worker, not every independent file/bridge entrypoint.
An absent sidecar never certifies that no decisions happened.

## Trace and replay

```sh
codescribe trace SESSION_ID --word Iwo
codescribe trace /explicit/test/session.trail.jsonl --word Iwo
```

Pins are enumerated by capture epoch, owner occurrence and sample interval;
equal labels never merge different physical pins. Fate is `introduced`,
`retained`, `changed`, or `lost_or_resegmented`. The last label does **not**
assert acoustic deletion: a changed boundary needs further lineage evidence.
An unpinned hypothesis is explicit. The engine-literal view is diagnostic and
is not a product Raw mode. Word filtering also finds a lost source word even
when it is absent from the candidate and result.

`replay_decisions` reads saved qualification/admission inputs and invokes
production `qualify` and `admit_with_slots`, checking receipts and pins before
and after. Its callback feeds those real receipts to the production reducer.
The synthetic app test persists five independent occurrences, reloads the
file and reproduces five ledger entries and five reducer entries. It decodes
no WAV. Unsupported corridors, projection records, mismatched pins, active
session sinks, schema/identity mismatches and missing end records fail
explicitly. General replay of a complete live take remains open; the synthetic
admission replay is the implemented subset. `production_replay.rs` remains a
different operation: rerunning today's engine on retained audio.

## Delivery corridors

| Surface                                     | Source corridor and destination                                                                                                                                                            |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Dictation / formatted dictation paste       | `controller/mod.rs`: stop canvas and terminal paths; `delivery_route::resolve_delivery_route` selects clipboard paste, clipboard hold, deferred insert or archive                          |
| Agent voice / assistive                     | `controller/helpers.rs`: voice turn; runtime session plus `ThreadDeliveryGateway`, `agent_delivery` broadcast; assistive provider; the delivery envelope preserves context references      |
| Agent channel                               | `controller/agent_channel.rs`: channel session seals utterances, archives and delivers to the declared binding; reopening is not a second microphone                                       |
| Composer dictation                          | `controller/mod.rs::format_composer_turn_once` uses the authenticated terminal source and one formatter call before composer delivery; `bridge/src/hotkeys.rs` forwards the app-owned take |
| Typed composer                              | `bridge/src/agent.rs`, agent runtime/session and thread gateway; typed/voice attachment conversion shares `core/attachment.rs`                                                             |
| Overlay To Agent                            | `DeliveryIntent::OverlayToAgent`, controller and bridge hotkey commands; resolves an explicit Agent route                                                                                  |
| Overlay Insert / Copy                       | `DeliveryIntent::OverlayInsert`, deferred insert and clipboard APIs; Copy observes the admitted rendered revision                                                                          |
| Overlay Aa / retranscribe / history restore | controller authenticated revision entrypoints → `PresentationEmitter` → reducer/Bus/delivery buffer; changes the existing take projection                                                  |
| Max                                         | `app/agent/max_consultation.rs`, `FormattingConsultation` and `ApplyConsultationPresentation`; whole/group presentation, not word-slot admission                                           |
| Quick Notes save only                       | `DeliveryIntent::NotesOnly` → archive only                                                                                                                                                 |
| CLI file / live reader                      | `bin/codescribe.rs`: file verdict delivery and optional Bus publication; live reader observes the app-owned Bus                                                                            |

The destination authority is `delivery_route.rs`; OS focus can hold a paste but
cannot choose the transcript or destination. This inventory does not assert
that Agent already receives the new Raw contract; L2/L4 own that change.

## Token, text and marker removal surfaces

| Surface                                                            | Current operation / implication                                                                                                                                                                                                                                           |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `acoustic_ledger::admit_with_slots`                                | Replaces held slots wholesale with supplied reconciled slots; an omitted pin can disappear; falsifier B exercises this exact mutation seam                                                                                                                                |
| `CommittedObservation::from_label`, `decide_observation`           | Whole-label correction may replace per-word slots with a label-wide slot; Formatter rank exceeds acoustic producers                                                                                                                                                       |
| `admit_word_slots`, `same_pcm_slot`                                | Removes conflicting slots when reconciling a higher-authority incoming word; records replacement refusals; adjacent/overlapping pins need acoustic lineage scrutiny                                                                                                       |
| `apple_live_session` reconciliation                                | Selects current Apple words, replaces/supersedes same PCM-slot evidence, empties pending queues and accounts stale/replaced returns; inspect `reconcile_silero_ledger`, Apple word selection, `complete_whisper_window`, formatter completions and explicit refusal exits |
| `light_plus::collapse_tokens` / `is_hesitation`                    | Deletes mixed-sentence hesitation tokens, including `yyy`; normalizes whitespace and punctuation. Repeated spoken words are no longer collapsed here                                                                                                                      |
| `ai_formatting::has_repetition_loop` / `remove_simple_repetitions` | Removes repeated one-to-three-word patterns before LLM execution; failure output can still be this cleaned input                                                                                                                                                          |
| Live Formatter proposal                                            | `TranscriptReducer::apply_occurrence_label_proposal` admits a whole occurrence label through the ledger; omitted words are not protected by the prompt                                                                                                                    |
| Whole document Formatter / Light+ / user / Max                     | `commit_document_revision`, `apply_user_revision`, `ApplyConsultationPresentation` can replace a shared rendered projection even when word slots remain unchanged                                                                                                         |
| Whisper `merge_chunk_transcripts`                                  | Keeps new segments only if their end exceeds the previous overlap boundary; requires timestamp provenance                                                                                                                                                                 |
| Whisper timestamp extraction                                       | Removes model special/timestamp tokens from decoded text; leaves trailing unclosed text out of timestamped segments while retaining the separate full-text result                                                                                                         |
| Whisper word punctuation merge                                     | Clears punctuation-only word objects after merging their content/range into a neighbour, then retains nonempty words                                                                                                                                                      |
| Whisper decoder / VAD / tail patcher                               | Suppresses special/blank tokens and limits decoding; VAD trims PCM, not text. Tail patcher alignment normalizes text keys but explicitly refuses shorter destructive replacements. These are separate from formatter authority                                            |
| Reducer visible canvas                                             | Filters covered unanchored alternatives from the main canvas, invalidates stale shapes, and changes lifecycle evidence visibility; the evidence book remains distinct from delivery                                                                                       |
| `context_bucket::strip_markers_for_delivery`                       | Removes only minted `{selection_N}` / `{image_N}` references on non-agent outward routes and repairs spacing; Agent preserves them                                                                                                                                        |
| `attachment::parse_image_attachment_block`                         | Removes the attachment-path marker block/separator while materializing provider image content; explicit privacy/input conversion boundary                                                                                                                                 |
| Thread title extraction (Rust and Swift)                           | `strip_context_markers`, title bullet/wrapping removal and length limits affect titles, not transcript identity                                                                                                                                                           |
| Bus retention/compaction                                           | Removes aged diagnostic evidence, not admitted delivery bytes; history readers select revisions rather than rewriting words                                                                                                                                               |

Literal review covered the Rust pipeline/STT/formatter/controller and Swift
Agent/overlay surfaces. This is a scoped source inventory; opaque provider/model
internals and user-configured custom prompts are not statically enumerable.

## Formatter backends and budgets

All configured cloud providers enter `core/llm/ai_formatting.rs`: OpenAI
Responses, xAI Responses, Libraxis Responses, Anthropic Messages and configured
custom provider references/wire families (`core/llm/provider.rs`). Max executes
through the host-owned formatting agent/consultation executor. Off skips.

Apple FoundationModels is also an LLM formatter: `core/llm/on_device.rs` plus
`bridge/src/on_device_format.rs` and `OnDeviceFormatterHost.swift`. Selection
uses `CODESCRIBE_FORMAT_ON_DEVICE`; the host registers its formatter. The funnel
tries that host before the configured cloud attempt. Its direct `format().await`
is outside the cloud call timeout wrapper. L1 does not change this behavior.
Polish support claims are not revalidated by the SoundAnalysis spike, which is
a different framework/model.

There **are** existing deadlines, but no single verified deadline bounds all
Stop → final delivery work:

- `controller/mod.rs::STOP_FINAL_BOUND`: 2 seconds from Stop for waiting on live
  finals/canvas. It explicitly excludes refinement, archive and formatter work;
  it is not the proposed final Light+ tick budget.
- `STOP_TIMEOUT` and `await_capture_stop`: bound a caller's wait for settlement;
  the owned settlement task can continue.
- `apple_live_session`: the live refinement stop deadline, task accounting,
  bounded joins and timeouts are distinct from the final projection.
- `ai_formatting`: configured attempt and inter-chunk timeouts plus bounded
  retries. Streaming initial readiness and stalled-stream bounds differ from a
  whole take deadline. On-device execution needs separate timeout scrutiny.
- Terminal composer formatting and explicit overlay formatting await this
  funnel; no common complete Stop-to-delivery budget was found in those paths.

L4 must establish the Smart deadline and delivery receipt; W-0b must measure the
entire tick corridor (queue, execution and accepted commit), not only `apply`.
