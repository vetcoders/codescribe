# CodeScribe Postmortem: What The App Gives Today vs What It Could Give

Date: 2026-04-12

Scope:
- Corpus review of `/Users/polyversai/.codescribe/transcriptions`
- Runtime audit against `/Users/polyversai/.codescribe/logs/codescribe.log`
- Spot-check adjudication with `codescribe transcribe <wav>`

Assumption for this report:
- Treat the engine as if it always had the capability.
- Focus on product routing, adjudication, post-processing, and user-facing truth.

## Corpus Snapshot

- `724` raw transcript files
- `321` paired raw WAV files
- `96` `*_ai.txt` files
- `24` `*_ai-failed.txt` files
- `14` `*_raw_1.txt` files

Outlier reality:
- `293` WAV/TXT pairs had enough data to score by duration vs transcript size.
- `170` pairs are `>=20s`.
- Only `5` pairs are both `>=20s` and `<100 chars`.
- Only `2` pairs are both `>=30s` and `<50 chars`.

Takeaway:
- The "big WAV -> tiny TXT" problem is real, but it is not the dominant mode of the product.
- The sharper problem is not broad engine failure. It is selective product misrouting and poor truth signaling on edge cases.

## Current State

### 1. The app does not have one transcript truth

The controller explicitly prefers different sources depending on runtime conditions:

- By default, streaming is the source of truth for final output.
- Final-pass local STT from the saved WAV exists, but is disabled by default behind `CODESCRIBE_LOCAL_STT_FINAL_PASS=1`.
- Cloud failure can force a fallback to streaming.

Code evidence:
- [app/controller/mod.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/mod.rs:2600)
- [app/controller/mod.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/mod.rs:2678)
- [core/stt/whisper/singleton.rs](/Users/polyversai/Libraxis/CodeScribe/core/stt/whisper/singleton.rs:201)

What the user gets today:
- A single pasted/saved text with no visible declaration of whether it came from streaming, final-pass local STT, or cloud fallback.

What the app could give:
- A source-aware result:
  - `streaming draft`
  - `final-pass local verdict`
  - `cloud result`
  - `no speech`
  - `low confidence`

### 2. The product already knows when audio is mostly silence, but hides that truth

The local file transcription path uses Silero VAD as a hard prefilter and returns empty when it finds no speech:

- [core/stt/whisper/singleton.rs](/Users/polyversai/Libraxis/CodeScribe/core/stt/whisper/singleton.rs:206)
- [core/stt/whisper/singleton.rs](/Users/polyversai/Libraxis/CodeScribe/core/stt/whisper/singleton.rs:216)

But the user-facing archive and pasted output do not preserve that verdict. They preserve only the final text artifact.

Historical evidence:
- `192407_to-co`:
  - log shows `2.0s speech / 31.6s total (6% speech)`
  - log also shows `Low avg logprob (-1.03) - possible hallucination`
  - product still saved and pasted `To, co,`
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:21454)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:21455)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:21470)

- `234950_panam-swoj-dzieki`:
  - historical runtime used `streaming transcription result`
  - fresh CLI verdict now says `No speech detected by Silero VAD`
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29391)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29392)
    - saved file: [234950_panam-swoj-dzieki_raw.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-10/234950_panam-swoj-dzieki_raw.txt:1)

What the user gets today:
- A short transcript that looks authoritative.

What the app could give:
- "Silero saw 0-6% speech in this clip."
- "This output is low-confidence / possible hallucination."
- "Nothing reliable to paste."

### 3. Cloud failure currently degrades to streaming truth without making the downgrade legible

Two sharp examples:

- `234950_panam-swoj-dzieki`
  - cloud failed
  - app fell back to streaming
  - user got the streaming text as final truth
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29390)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29392)

- `235906_ale-sluchaj-marbles`
  - cloud failed
  - app fell back to streaming
  - saved output is garbled/code-mixed
  - fresh CLI adjudication on the same WAV is materially different and somewhat cleaner
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29439)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29441)
    - saved file: [235906_ale-sluchaj-marbles_raw.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-10/235906_ale-sluchaj-marbles_raw.txt:1)

What the user gets today:
- A single output that hides the fact that it is a degraded fallback path.

What the app could give:
- A clear downgrade banner:
  - `cloud failed`
  - `showing streaming fallback`
  - `run local adjudication before paste?`

### 4. Post-processing sometimes makes a better transcript worse

The best historical proof is the `raw` vs `raw_1` pairs.

Examples:
- [212048_co-bede-robil_raw.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-08/212048_co-bede-robil_raw.txt:1)
- [212048_co-bede-robil_raw_1.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-08/212048_co-bede-robil_raw_1.txt:1)
- [212242_zastanawiam-sie-co_raw.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-08/212242_zastanawiam-sie-co_raw.txt:1)
- [212242_zastanawiam-sie-co_raw_1.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-08/212242_zastanawiam-sie-co_raw_1.txt:1)

Historical truth from logs:
- For both samples, the controller explicitly used `final-pass local transcription result`.
- Then post-processing introduced a one-token delta and saved the later `raw_1`.
- In practice, the cleaner raw was not the final user-facing truth.
- refs:
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23513)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23516)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23520)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23535)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23585)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23588)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23590)
  - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:23605)

What the user gets today:
- The corrected version, even when the correction is actually degradation.

What the app could give:
- A correction guard that compares `raw` vs `postprocessed` and refuses to overwrite when the "correction" introduces suspicious foreign tokens or lowers confidence.

### 5. AI is powerful, but the product mixes categories

There are two very different AI surfaces in the archive:

- `assistive` expansion:
  - raw transcript becomes a much larger synthesized artifact
  - example: `013147_i-dont-have_raw.txt` -> `013147_i-dont-have_ai.txt`
  - example: `234846_emil-courier-bo_raw.txt` -> `234846_emil-courier-bo_ai.txt`
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:17379)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:17405)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:19671)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:19697)

- formatting failure fallback:
  - the file suffix says `ai-failed`
  - but the saved text is often still usable cleaned output, not a transparent failure message
  - examples:
    - [193710_kanal-oss-jest_ai-failed.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-10/193710_kanal-oss-jest_ai-failed.txt:1)
    - [000914_i-can-do_ai-failed.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-11/000914_i-can-do_ai-failed.txt:1)
    - [001954_thats-not-the_ai-failed.txt](/Users/polyversai/.codescribe/transcriptions/2026-04-11/001954_thats-not-the_ai-failed.txt:1)
  - refs:
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29164)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29177)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29509)
    - [codescribe.log](/Users/polyversai/.codescribe/logs/codescribe.log:29523)

There is also a contract gap in code:
- The default formatting prompt forbids adding new meaning.
- The implementation only really guards against refusal and raw echo, not semantic drift.
- refs:
  - [core/config/prompts.rs](/Users/polyversai/Libraxis/CodeScribe/core/config/prompts.rs:7)
  - [core/config/prompts.rs](/Users/polyversai/Libraxis/CodeScribe/core/config/prompts.rs:19)
  - [core/llm/ai_formatting.rs](/Users/polyversai/Libraxis/CodeScribe/core/llm/ai_formatting.rs:1011)
  - [core/llm/ai_formatting.rs](/Users/polyversai/Libraxis/CodeScribe/core/llm/ai_formatting.rs:1069)

What the user gets today:
- Sometimes a transcript.
- Sometimes an assistant artifact.
- Sometimes a formatting fallback mislabeled as failure.

What the app could give:
- A clean product split:
  - `Transcript`
  - `Formatted transcript`
  - `Assistant interpretation`
  - `Formatting failed, raw preserved`

## What The App Gives Today

The current app gives the user:

- Fast dictation flow with real paste-to-app ergonomics.
- Archival raw transcripts and sometimes paired WAVs.
- Optional AI surfaces that can produce genuinely useful expansions.
- Good handling on dense, speech-rich clips.

But it also gives the user false certainty in edge cases:

- It hides transcript source selection.
- It hides VAD truth.
- It hides low-confidence warnings.
- It sometimes pastes fallback or degraded text as if it were clean truth.
- It mixes transcripting and interpreting under file names that do not explain the distinction.

## What The App Could Give

If we assume the engine always could, then the missing value is productized adjudication.

The app could give:

- A two-step truth model:
  - `what we heard`
  - `how sure we are`

- A source-aware final result:
  - `streaming draft`
  - `final-pass local`
  - `cloud`
  - `fallback`

- Explicit no-speech outcomes:
  - save/paste nothing when VAD says no speech
  - or save a verdict artifact instead of fake text

- Confidence-aware blocking:
  - do not auto-paste low-logprob hallucination candidates
  - require manual confirm for suspicious outputs

- Correction discipline:
  - never overwrite a cleaner raw transcript with a worse postprocessed one

- AI category clarity:
  - transcript vs formatter vs assistant should be different product surfaces, not just different suffixes

## Proposal

The better shape is:

1. Final-pass local STT becomes the adjudicator whenever a WAV exists.
2. Streaming remains a live UX draft, not silent final truth.
3. VAD/confidence/source become first-class metadata shown to the user and stored with the artifact.
4. Postprocess and AI are treated as optional transforms on top of an adjudicated transcript, never as hidden replacements.

## Is The Bottleneck In `app` Or `core`?

Short answer:
- Mostly `app`.
- Not exclusively `app`.

Why it is mostly `app`:
- `core` already emits useful runtime truth:
  - `NoSpeech`
  - hallucination drops
  - semantic gate drops
  - session stats
- `app` already captures that telemetry for routing decisions:
  - [app/controller/helpers.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/helpers.rs:765)
  - [app/controller/helpers.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/helpers.rs:807)
- But `app` still:
  - chooses between transcript sources opaquely
  - pastes fallback text as if it were authoritative
  - saves degraded postprocessed text over cleaner raw
  - hides `NoSpeech` and most quality signals from the user
  - refs:
    - [app/controller/mod.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/mod.rs:2655)
    - [app/controller/mod.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/mod.rs:2816)
    - [app/controller/mod.rs](/Users/polyversai/Libraxis/CodeScribe/app/controller/mod.rs:3192)

Why `core` is not entirely innocent:
- The final-pass file API still collapses adjudication down to plain text.
- `transcribe_file()` logs VAD stats and returns empty on no-speech, but it returns only `String`, not a structured verdict.
- `RawTranscript` contains only `text` and `segments`, so important confidence metadata is lost at the API boundary.
- Low average logprob is only a warning in logs, not part of the returned contract.
- refs:
  - [core/stt/whisper/singleton.rs](/Users/polyversai/Libraxis/CodeScribe/core/stt/whisper/singleton.rs:201)
  - [core/pipeline/contracts.rs](/Users/polyversai/Libraxis/CodeScribe/core/pipeline/contracts.rs:45)
  - [core/stt/whisper/engine.rs](/Users/polyversai/Libraxis/CodeScribe/core/stt/whisper/engine.rs:861)

Practical verdict:
- The product throttling is primarily in `app`, because that is where source selection, fallback policy, postprocess overwrite, and paste/save decisions happen.
- The cleanest long-term fix still requires one small `core` contract upgrade:
  - return structured adjudication metadata, not just text.

## Migration Plan

1. Promote transcript provenance to a persisted artifact.
   - Store source (`streaming`, `local_final_pass`, `cloud`, `fallback`) plus VAD stats and confidence hints next to every saved transcript.

2. Change final selection policy.
   - If WAV exists, run local final-pass adjudication by default.
   - If it returns empty/no-speech, do not paste the streaming fallback silently.

3. Add a low-confidence gate.
   - If Whisper raises low logprob on final-pass, surface `possible hallucination`.
   - Downgrade to manual confirmation instead of auto-paste.

4. Split transcript surfaces.
   - `raw transcript`
   - `formatted transcript`
   - `assistant output`
   - `ai-failed fallback`

5. Add correction rollback.
   - If postprocess introduces suspect token drift, keep raw and suppress the rewrite.

## Quick Win

The smallest sharp move with the highest leverage:

- Make final-pass local STT the default adjudicator whenever the app has a WAV.
- Surface one small badge in the UI and archive:
  - `Final-pass local`
  - `Streaming fallback`
  - `No speech`
  - `Low confidence`

That one move would not make the engine better.
It would make the product finally tell the truth about what the engine already knew.
