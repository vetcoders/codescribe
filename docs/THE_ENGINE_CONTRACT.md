# THE ENGINE — quality-report contract

| | |
| --- | --- |
| id | `the-engine/v1` |
| corpus schema | `codescribe-corpus-parity/v3` |
| primary key | `pcm_time` |
| source of truth | `core/quality/engine_contract.rs` |
| surfaces | Qube `index.html`, `codescribe-corpus` private quality HTML, Teacher HTML |

Do not re-derive this. If a sentence here disagrees with `ENGINE_CONTRACT` in Rust, the Rust constant wins and this file is wrong.

## Product goal

Place on the canvas is given by **energy in time** — mechanical waves from the speaker's vocal cords, while they speak. Not by tokens.

Four live layers, everything in the buffer, **~10 ms to paste**:

> energy × time → the true sentence, live in the buffer, ~10ms to paste

Preview, colours, successive hypotheses and seals are internal mechanics. The user buys the sentence, immediately, ready to paste. 20 seconds of delay kills even a perfect transcript: it is no longer presence.

## Relay

Apple → Whisper → lexicon → formatter → human

This is a band, not a queue of correctors. Ban is **per layer, per span**. The layer that already passed this span is out. The next one may enrich the **same** time window.

- **Apple** draws now (thin, sharp pencil). Span commits → Apple out.
- **Whisper** enters the buffer on **3-5s utterance-bounded partials**. Never full audio in the automatic pipeline (`full_file_pass = button_only_proposal`). Must not hallucinate into silence. Excess recall is stuffed into holes Apple left (`ReplaceRange`, never full-replace).
- **Lexicon / Light+** tune after Whisper settles.
- **Formatter** (Responses, `previous_response_id`) has a trash bucket. It may throw away. It may not rearrange the plate.
- **Human** is last, after seal.

`NEVER REWRITE FROM ZERO.` Append-only. Key = PCM sample counter, not token position.

## Three bars — not synonyms

| Bar | Means |
| --- | --- |
| `utterance_final` / committed | This layer finished its hypothesis for the fragment. That layer is banned from further overwrite of this span. Preview grey, committed bright. **Not the document.** |
| `utterance_sealed` | Apple + Whisper + lexicon finished fusion for the Silero-bounded span. Record `[sample_start, sample_end)` becomes append-only and may start inline formatting. Order on the PCM axis is frozen. |
| `transcript_sealed` | The whole session — tail and formatter included — was assembled into the document. Automation puts its hands down. Full HQ / Cloud may only propose a variant. |

`committed` does **not** mean "this is already the document". It means: **this layer finished its work here; the next layer takes the same time slice.**

## What is *not* true

Before `transcript_sealed` the whole document is **not** mutable.

- Closed spans stay on their places on the PCM axis.
- Utterances may not be reordered. Text may not be built from zero.
- The current tail may still evolve.
- Whisper may fill holes and replace weaker evidence inside a still-unsealed allowed span.
- The formatter works in parallel on closed fragments and keeps their order.
- Stop closes only the tail and assembles ready fragments.

A first-wins final string is not enough. The real document is the ordered span ledger with provenance. Session seal closes the assembled result — it does not replace the architecture with one frozen variable.

## Quality-report column roles

| Column | Role |
| --- | --- |
| `raw` / `live` | live hypothesis |
| `post` / `layer1` | Whisper hole-fill |
| `sealed` | sealed span |
| `delivered` / `session` | session document |
| `ai` / `cloud` / `hq` | `HumanTriggeredProposal` — WER against a proposal does not promote it to document |

## Forbidden (reports and agents)

- `rewrite_from_zero`
- `reorder_spans`
- `hallucinate_into_silence`
- `full_file_in_automatic_pipeline`
- `auto_replace_after_transcript_sealed`
- `treat_committed_as_document`
- `treat_whole_text_mutable_until_session_seal`

## How a quality HTML must behave

1. Embed the contract plate from `render_engine_contract_html()` before scores.
2. Carry `data-contract="the-engine/v1"` and `data-primary-key="pcm_time"`.
3. Treat Cloud / HQ / AI-formatted as proposals.
4. Never present WER as if a full-file pass were the live engine.
5. `codescribe-corpus` machine JSON stays fail-closed on privacy (no source paths, no transcript bodies). The private Qube HTML may show bodies and is the only place the plate sits next to audio.
