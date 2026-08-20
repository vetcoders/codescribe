# THE ENGINE — quality-report contract

|                 |                                                                                                        |
| --------------- | ------------------------------------------------------------------------------------------------------ |
| id              | `the-engine/v1`                                                                                        |
| corpus schema   | `codescribe-corpus-parity/v3`                                                                          |
| primary key     | `pcm_time`                                                                                             |
| source of truth | `core/quality/engine_contract.rs`                                                                      |
| surfaces        | **Seal Atlas** in Voice Lab (`voice-lab` tab); gold HTML `docs/quality-reports/seal-atlas.take01.html` |

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

| Bar                           | Means                                                                                                                                                                                            |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `utterance_final` / committed | This layer finished its hypothesis for the fragment. That layer is banned from further overwrite of this span. Preview grey, committed bright. **Not the document.**                             |
| `utterance_sealed`            | Apple + Whisper + lexicon finished fusion for the Silero-bounded span. Record `[sample_start, sample_end)` becomes append-only and may start inline formatting. Order on the PCM axis is frozen. |
| `transcript_sealed`           | The whole session — tail and formatter included — was assembled into the document. Automation puts its hands down. Full HQ / Cloud may only propose a variant.                                   |

`committed` does **not** mean "this is already the document". It means: **this layer finished its work here; the next layer takes the same time slice.**

## What is _not_ true

Before `transcript_sealed` the whole document is **not** mutable.

- Closed spans stay on their places on the PCM axis.
- Utterances may not be reordered. Text may not be built from zero.
- The current tail may still evolve.
- Whisper may fill holes and replace weaker evidence inside a still-unsealed allowed span.
- The formatter works in parallel on closed fragments and keeps their order.
- Stop closes only the tail and assembles ready fragments.

A first-wins final string is not enough. The real document is the ordered span ledger with provenance. Session seal closes the assembled result — it does not replace the architecture with one frozen variable.

## Quality-report column roles

| Column                  | Role                                                                              |
| ----------------------- | --------------------------------------------------------------------------------- |
| `raw` / `live`          | live hypothesis                                                                   |
| `post` / `layer1`       | Whisper hole-fill                                                                 |
| `sealed`                | sealed span                                                                       |
| `delivered` / `session` | session document                                                                  |
| `ai` / `cloud` / `hq`   | `HumanTriggeredProposal` — WER against a proposal does not promote it to document |

## Forbidden (reports and agents)

- `rewrite_from_zero`
- `reorder_spans`
- `hallucinate_into_silence`
- `full_file_in_automatic_pipeline`
- `auto_replace_after_transcript_sealed`
- `treat_committed_as_document`
- `treat_whole_text_mutable_until_session_seal`

## How a quality HTML must behave

The private quality HTML is a **Seal Atlas**, not a WER table with a banner.

Gold visual: [`docs/quality-reports/seal-atlas.take01.html`](quality-reports/seal-atlas.take01.html)
(take 01 „no to dobra", replay `cff0817b…`, 44100 Hz, 2650112 samples).

HTML handshake (meta, `.stat` cards, lanes): [`docs/quality-reports/CONTRACT.md`](quality-reports/CONTRACT.md).
`codescribe-corpus` writes `quality/seal-atlas.{profile}.html`. Qube is a footnote.

1. **One take, one clock.** X-axis is the capture PCM sample counter. Apple sealed spans, `SealedSpan.words`, Whisper segments, and Silero p(speech) share that axis. No reconstructed timeline.
2. **Production Silero.** Curve from the engine's embedded ONNX + default `VadConfig`, 32 ms chunks — `vad_atlas_probe`, not a toy VAD.
3. **Words from the live dump.** `CODESCRIBE_SEAL_ATLAS_DUMP` after session-end seal. Never rebuilt from the final string.
4. **Two grain classes, labeled.**
   - _word-grain_ — SFSpeech actually returned per-word pins (take 01: spans 3 and 9).
   - _utterance-grain_ — one “word” covering the whole Apple commit-to-commit window. Per-word payload is real where it exists and **not guaranteed**.
5. **Clock-lie** (`clock-lie`) is a first-class finding. Span 2 of take 01: 41 characters pinned to 100 ms (410 chars/s). Physically impossible. Silero can still say “speech 100%” — the range is not the range of that speech. Same class as `seal window unresolved`.
6. **Utterance-grain includes silence tails.** Span 8: 36% speech. The span range is the distance between Apple commits, not the speech outline. This is why W13-3B mints identity from Silero silence edges.
7. **Letters = interpolation.** Grapheme ticks inside a word are an even split of the word range, drawn as such. Not a measurement. Forced-aligner would be required for real grapheme times — we do not pretend to have one.
8. Whisper is drawn as `whisper_words` mapped **back** onto the same clock. HQ / Cloud / AI-formatted stay proposals.
9. `codescribe-corpus` machine JSON stays fail-closed on privacy. The private Atlas HTML is the only place bodies sit next to audio.
10. A WER table may exist as a footnote. It must not replace the atlas and must not present a full-file pass as the live engine.

## Voice Lab handshake

The operator does not open these HTML files from Finder. **Voice Lab**
(`vetcoders-tools` / `voice-lab`) is the console. Tab **Seal Atlas** lists
private HTML under:

`~/.vibecrafted/artifacts/vetcoders/codescribe` (`CODESCRIBE_ARTIFACTS_DIR`)

`codescribe-corpus` must write atlas HTML **into that tree**. Voice Lab then:

1. Classifies by title + relative path (case-insensitive):
   - contains `seal atlas` or `seal-atlas` → kind `seal_atlas` (sorted first)
   - contains `quality` **and** `contract` → kind `quality_contract`
   - else → `quality_report` (the old Qube WER page lands here — not the throne)
2. Pulls fact cards from `<div class="stat"><b>VALUE</b><span>LABEL</span></div>`
   (take 01: 20/20 word-grain, 11 spans, 2 per-word, 1 clock-lie, Silero 0.5).
3. Iframes the file (`sandbox=allow-scripts`). Absolute paths never leave the
   catalog JSON.

A report that fails this handshake is invisible in the lab, whatever its WER.
The gold take 01 HTML already satisfies it. Qube `Codescribe Quality Report`
does not — that title is the thing we are retiring.

## Supervisor findings

Source of truth: `core/quality/supervisor.rs`.
Schema: `codescribe-supervisor-findings/v1`.

Voice Lab three-judge emits a `supervisor` object next to the WER footnote.
Daily is the session document. Candle HQ and cloud `:8444` stay
`HumanTriggeredProposal`. WER is agreement with a proposal, not accuracy.

A finding is only a finding when it names:

- `kind` from the engine catalog
- `claim` that can be false
- `falsifier` — what would disprove it
- `action` — engine cut, lexicon tune, lab-judge hygiene, or operator review

Judge hygiene kinds the lab used to commit (these are P0/P1 when they fire):

- `hq_treated_as_document`
- `cloud_treated_as_document`
- `wer_promoted_to_document_score`
- `omitted_programming_vocabulary`
- `last_session_paired_with_live_overlay`
- `leftover_websocket_polarity`
- `proposal_agreement_misread_as_accuracy`

The catalog also names every engine-side class already in the tree (contract
forbiddens, clock-lie, speech_gap, Teacher attention, confidence flags,
delivery gates, Whisper silence residue). Missing evidence does not invent a
hit. Relative-zero FP starts here: do not crown HQ, do not omit
`vocabulary=programming` on `:8444`, do not pair a live overlay with the
previous `last_session.wav`.
