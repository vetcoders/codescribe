# CodeScribe — Diagnoza: top strzały (hands-off append + Format)

> Sesja 2026-05-28/30 · branch `fix/toggle-stuck-watchdog` · audytor: claude (Opus 4.8)
> Metoda: git show/blame + Read + grep + operator screen-recording. Read-only diagnostyka.
> Każdy strzał: **Symbole** (file:line) · **Evidence** · **Pewność** · **Jak potwierdzić/obalić**.

---

## Co zostało WYKLUCZONE (zweryfikowane jako niewinne)

Ważne, bo zawęża pole. Te miejsca **działają poprawnie** — nie tu szukać:

| Symbol | file:line | Dowód niewinności |
|---|---|---|
| `append_transcription_delta_impl` | `app/ui/overlay/mod.rs:1434` | `TranscriptDelta::from_raw(delta).apply(&mut accumulated_text)` — kumulatywny backspace-protokół, poprawny |
| `append_voice_chat_user_delta_impl` | `app/ui/voice_chat/api.rs:1080` | identyczny `TranscriptDelta::apply(&mut msg.text)` — Emil append też poprawny |
| `route_transcription_delta` | `app/controller/helpers.rs:66` | routuje na append-path, kontrakt zakazuje full-snapshotów |
| `set_transcription_text` (replace) | `app/ui/overlay/mod.rs:1477` | **0 callerów** w `app/` — nikt nie klobruje appendów |
| `SessionState` math (`apply_preview`/`finalize`/`rendered_text`) | `emitter.rs:64/86/127` | **unit-tested**: `session_state_appends_preview_after_committed_text` (emitter:455), `stats_clears_uncommitted_preview_after_finalized_utterance` (emitter:684) — kumulacja działa w izolacji |

**Wniosek:** logika appendu i matematyka kumulacji są poprawne. Bug NIE jest w „jak dodajemy", tylko w „czy `committed` w ogóle rośnie w czasie sesji" — poziom eventów / lifecycle, nie matematyka.

---

## TOP STRZAŁ #1 — append-replace (R05 / operator #1 CRITICAL)

**Teza:** w streaming-toggle `committed` zostaje puste przez całą sesję, więc
`rendered_text() = committed + active_preview` pokazuje tylko NAJŚWIEŻSZY preview →
efekt podmiany zamiast appendu. Operator @02:04: *„w bufferze dobrze, UI nie umie
dodawać na końcu ogona"* — rozdziela transcript_buffer (OK) od live-render (zły).

**Symbole pod lupą:**
- `PresentationEmitter::on_event` → branch `EngineEvent::UtteranceFinal` — `app/presentation/emitter.rs:332`
- duplicate-guard `last_dispatched_utterance_id.swap(...)` — `emitter.rs:341–351` ← **podejrzany: może połykać finale**
- `SessionState::finalize` (preview→committed) — `emitter.rs:86` (wywoływane TYLKO z UtteranceFinal:354)
- `SessionState::rendered_text` — `emitter.rs:127`
- emisja źródłowa: `event_sink.on_event(&EngineEvent::UtteranceFinal {...})` — `core/pipeline/streaming.rs:1725` i `:1851`, za `SpeechEvent::UtteranceFinal` (`streaming.rs:1266`)

**Trzy konkretne hipotezy (malejąca pewność):**
1. **UtteranceFinal nie odpala podczas ciągłego mówienia** (mało pauz VAD) → `finalize` nigdy nie woła → `committed` puste. **MEDIUM.**
2. **Duplicate-guard (emitter:341) połyka finale** — jeśli `utterance_id` nie inkrementuje per-utterance (np. zostaje 0), `swap` zwraca „duplicate" i `return` (emitter:350) pomija `finalize`. **MEDIUM.**
3. **`PresentationEmitter`/`SessionState` recreowany mid-sesja** — `configure_toggle_event_sink` woła `PresentationEmitter::new` i jest wywoływany 2× (`controller.rs:2768`, `:2790`); jeśli stan się resetuje, `committed` znika. **LOW-MEDIUM.**

**Pewność łączna:** warstwa = **HIGH** (to emitter event-cadence/lifecycle, nie append-impl ani SessionState-math). Mechanizm = **MEDIUM** (która z 1/2/3).

**Jak potwierdzić (jeden ruch):** headless test — podać do `PresentationEmitter::on_event` sekwencję `Preview("A") → UtteranceFinal(id=1,"A") → Preview("B") → UtteranceFinal(id=2,"B")` i zaasercować `rendered_text() == "A B"`. Jeśli przejdzie → bug w emisji STT (hipoteza 1) lub lifecycle (3); jeśli padnie z `utterance_id`=0 → to guard (hipoteza 2), czarno na białym. Zamyka też test-gap P2.

---

## TOP STRZAŁ #2 — Emil dziedziczy ten sam root cause

**Teza:** bąbel Emila podmienia z tego samego powodu co overlay — wspólny upstream
(emitter cadence), nie bug lokalny Emila.

**Evidence:** `append_voice_chat_user_delta_impl` (api.rs:1080) zweryfikowany jako
poprawny (patrz „wykluczone"). Obie powierzchnie karmione z `route_transcription_delta`
→ ten sam `BufferedEmitter`/`SessionState`. Operator @01:32–02:13: Emil też pokazuje
*„de facto ten sam efekt, podmianka"*.

**Pewność:** **HIGH**, że Emil = ta sama przyczyna co #1. Naprawa #1 naprawia i Emila.

---

## TOP STRZAŁ #3 — Format: fire-and-forget UX (zweryfikowane w pełni)

**Teza:** przycisk [Format] jest podpięty i działa, ale przepływ to fire-and-forget:
natychmiastowe zniknięcie overlaya, kilkusekundowa cicha luka (LLM), wklejenie w
niegwarantowany target, milczenie przy błędzie.

**Symbole + dokładny przepływ:**
- `on_format_transcript` — `app/ui/overlay/mod.rs:386` → woła `request_format_and_paste()` **a potem natychmiast** `hide_transcription_overlay()`
- `request_format_and_paste` — `app/controller/mod.rs:864` → `current_segment_text()` (sync, OK) → `tokio::spawn` → **brak capture target-app**
- `format_decision_text_and_paste` — `app/controller/mod.rs:3029` → `ai_formatting::format_text_with_status(text, lang, assistive=false, None)` → `clipboard::paste_text` → **warn-only przy błędzie**
- `format_text_with_status` — `core/llm/ai_formatting.rs:830` → `should_skip_ai_formatting`: **tekst < 24 znaki ⇒ zwraca surowy, status Skipped**
- `paste_text` → `paste_text_smart` — `app/os/clipboard.rs:386/300` → Cmd+V w frontmost app + restore schowka

**Wady (z kodu, nie spekulacja):**
- (A) zero feedbacku w trakcie — overlay znika sync, format leci async
- (B) target wklejenia nie złapany — wyścig o focus (Emil-path łapie, Format-path nie)
- (C) błędy ciche (`warn!`)
- (D) < 24 znaki = po cichu bez formatowania

**Pewność:** **HIGH** (przepływ prześledzony deterministycznie z kodu).

**Status:** mój cut dziś naprawił TYLKO label/tooltip/styl (`refresh_action_contract_ui_unlocked` tooltip, `decision_hint_text` header „Format · Copy · Agent", styl `GLASS→ROUNDED`). **Zachowanie A–D nietknięte** — to osobny, świadomy cut UX.

---

## Korekta proweniencji (operator miał rację)

Powierzchnia `[Format]/[Copy]/[Agent]` (`enter_decision_mode` controller:1529,
`show_decision_overlay` :1511, `set_transcription_action_contract` :4189) = **commit
`612c8260`, PRE-EXISTING**. c3ce222 zrobił tylko: rename labelek, on-demand
`request_format_and_paste`, `should_auto_paste=false` (:4137, blame `c3ce2220`),
auto-format off via `force_raw toggle_handsoff` (:3795, blame `c3ce2220`).
**Pewność: HIGH** (`git blame`). Mój pierwotny claim „c3ce222 wpiął kontrakt" był
błędny — cutoffflu (zaufanie diffowi+commit-msg zamiast blame).

---

## Ranking strzałów

| # | Cel | Warstwa | Pewność | Następny ruch |
|---|---|---|---|---|
| 1 | append-replace overlay (R05) | `emitter.rs` cadence/lifecycle | warstwa HIGH / mechanizm MEDIUM | headless test sekwencji eventów |
| 2 | Emil podmianka | = #1 (wspólny upstream) | HIGH | naprawa #1 |
| 3 | Format fire-and-forget UX | `on_format_transcript`/controller | HIGH | cut UX: feedback + capture target + close-after-done |
| — | proweniencja kontraktu | git history | HIGH | (zamknięte) |

**Najwyższa dźwignia:** strzał #1 headless-testem — deterministycznie wskaże który
z trzech mechanizmów, bez Twojego runtime, i zamknie test-gap P2. Naprawa #1 zamyka też #2.

---

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by VetCoders (c)2024-2026 LibraxisAI_
