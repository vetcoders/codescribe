# vc-audit — Faza 1: Ciągły Hands-Off Transcript

**Spec:** `docs/ADR/2026-05-28-Correction-Continuous-Hands-Off.md` (operator-authored, status PROPOSED)
**Impl:** commit `c3ce222` (`app/controller/mod.rs`, `app/ui/overlay/mod.rs`, `app/presentation/emitter.rs`)
**Runtime evidence:** `Screen_Recording_2026-05-28_at_14.47.14.mov` (362s) → `/tmp/faza1-screenscribe` (screenscribe v0.1.11 review, 33 transcript segments, 10 POI)
**Branch:** `fix/toggle-stuck-watchdog` · worktree clean · 4 ahead origin
**Date:** 2026-05-28

---

## EXECUTIVE VERDICT

### `STAGE_PARTIAL` — backend wylądował, prezentacja UI nie. Faza 1 **NIE zaakceptowana**; Faza 2 pozostaje **GATED**.

Implementacja Fazy 1 jest **architektonicznie poprawna w backendzie** (buffer
appenduje jeden ciągły transkrypt, pełne audio retencjonowane, ścieżka
per-utterance fizycznie usunięta, kontrakt akcji [Format]/[Copy]/[Agent]
wpięty). Ale **centralny user-visible cel — overlay rosnący przez append —
zawodzi w runtime**: warstwa prezentacji nadal **podmienia zamiast appendować**.

Sam operator na nagraniu rozdziela te warstwy chirurgicznie (@02:04):
> *"dzieje się w bufferze dobrze, a UI nie jest w stanie dodawać tekstu na końcu swojego ogona"*

i wystawia własny werdykt (@koniec):
> *"Sukces … połowiczny, ale czekamy na więcej, przynajmniej mamy pełne transkrypcje z trybu hands off"*

ADR § Akceptacja wymaga **jawnego potwierdzenia operatora**, że append faktycznie
działa. To potwierdzenie **nie padło** — padło "połowiczny". Niezmiennik 1
("Kolejność święta") trzyma więc Fazę 2 zamkniętą.

### Liczby

| Verdict | Count | Requirements |
|---|---|---|
| STAGE_PASS | 5 | R02, R03, R04, I04, NG01 |
| STAGE_PARTIAL | 2 | R01, B01 |
| STAGE_FAIL | 1 | R05 |
| NOT_ACCEPTED (gate) | 1 | A01 |

| Severity | Count | Items |
|---|---|---|
| P0 | 1 | A01 — acceptance gate nie przeszła |
| P1 | 2 | R05 (overlay nie appenduje), B01 (Emil podmienia) |
| P2 | 1 | test-gap: cała ścieżka stop za `cfg!(test)` |
| P3 | 0 | — |

### Top 5 ryzyk

1. **(P0) Rozdźwięk warstwowy buffer↔overlay.** Backend appenduje, UI renderuje podmianę. Dopóki overlay nie konsumuje `SessionRendered`/`rendered_text()` jako rosnącego tekstu, cel Fazy 1 jest niewidoczny dla użytkownika. (memory: `project_app_pipeline_spec` — "bug żyje w ./app NIE ./core").
2. **(P0) Faza 2 kuszą do startu.** Kod produkcyjny wygląda "zielono" (gates OK), co może mylnie zachęcić do ruszenia warstwowego modelu. ADR tego zakazuje przed akceptem.
3. **(P1) Emil dostaje podmiankę.** Assistive ścieżka ma poprawny single-call w kodzie, ale runtime pokazuje ten sam UI replace + niejasny timing wysyłki.
4. **(P2) Zero pokrycia testem realnej ścieżki.** `stop_toggle_and_adjudicate`→`cfg!(test)` short-circuit + `recorder.stop()` za `if !cfg!(test)`. Jedyny dowód = runtime operatora. Regresja appendu w UI przeszłaby przez wszystkie bramki niezauważona.
5. **(P3-info) Literalny `append_mode` nadal `false`.** Ciągłość realizowana równoważnym mechanizmem (`SessionRendered`), co ADR R2 dopuszcza — ale rozjazd litera-spec vs implementacja warto udokumentować.

### Następne 5 ruchów

1. **Napraw overlay append (P1, R05).** Zlokalizować, jak transcription overlay konsumuje delty — czy renderuje `Preview {rev,text}` (replace, utterance-local) zamiast skumulowanego `SessionRendered`/`rendered_text()`. To jest jedna naprawa w `app/ui/overlay` + `app/presentation/emitter.rs`, nie w `core`.
2. **Napraw Emil bubble append (P1, B01)** tym samym ruchem warstwowym.
3. **Dodaj headless test ścieżki stop (P2)** który nie chowa się za `cfg!(test)` — choćby na poziomie `rendered_text()` cumulative na sekwencji delt (test bez recordera/AppKit).
4. **Re-test runtime przez operatora** po naprawie UI → dopiero wtedy bramka A01 może przejść.
5. **NIE ruszać Fazy 2** do jawnego "tak, Faza 1 działa".

`model_confidence: high` — runtime evidence jest jednoznaczny (operator na nagraniu wprost rozdziela buffer-OK od UI-fail i nazywa wynik "połowicznym"); code evidence ze `git show` jest pełny, nie pochodny.

---

## Phase 1 — Context Receipt

- repo root: `/Users/maciejgad/vc-workspace/Vetcoders/Codescribe`
- branch: `fix/toggle-stuck-watchdog`, HEAD `e2152e1`, 4 ahead `origin`
- `dirty_worktree`: **false** (potwierdzone `repo-full` + SessionStart snapshot)
- snapshot: fresh (loctree SessionStart card, 258 files, 231 edges)
- hub-file w blast radius: `core/pipeline/contracts.rs` (19 importerów) — **NIE dotknięty** przez c3ce222 (zmiany w `app/`, zgodnie z layer-discipline CLAUDE.md)
- authority: code evidence = RepoVerified (git show); aicx watchdog-history = AicxAgent (tło, nie repo-fact); runtime = operator on record (najwyższe zaufanie dla acceptance)

## Phase 2 — Task Ingestion

| Task ID | File | Full read | Acceptance criteria | Stage notes |
|---|---|---|---|---|
| faza1 | `docs/ADR/2026-05-28-Correction-Continuous-Hands-Off.md` | FULL_READ | 5 (R01–R05) + 4 niezmienniki + gate A01 | Faza 1 landed-scope; Faza 2 deferred-by-design (gated) |

Status ADR = **PROPOSED** (claim, nie prawda — audit weryfikuje przeciw kodowi+runtime).

## Phase 3 — Requirements (9)

Pełne rekordy w `audit_requirements_matrix.jsonl`. Wyciąg: R01 ciągłość append · R02 brak per-utterance pipeline · R03 brak discard WAV · R04 full→process_stopped_recording · R05 overlay rosnący append · I04 akcje nie tryby · B01 Emil raz na całość · NG01 Faza 2 nietknięta · A01 acceptance gate.

## Phase 4–5 — Verification + Adversarial Pass

| Req | Positive (code) | Negative check | Runtime | Grade | Verdict |
|---|---|---|---|---|---|
| **R01** | per-utterance callback usunięty; `SessionRendered` default; single buffer | `handle_toggle_utterance` 0 refs | buffer OK, **UI replace** | MEDIUM / runtime-falsified(UI) | **STAGE_PARTIAL** |
| **R02** | `handle_toggle_utterance` skasowane (118 LOC) | 0 refs w app/+core/ | brak skarg na per-zdanie pipeline | STRONG | **STAGE_PASS** |
| **R03** | `recorder.stop()` (3146/2916) nie `stop_and_discard_path` | discard nie na stop-path | "mamy pełne transkrypcje z hands off" | STRONG | **STAGE_PASS** |
| **R04** | `process_stopped_recording(full, audio, ToggleSessionAdjudicated)` (3162) | brak per-utterance path | full text w decision overlay | STRONG | **STAGE_PASS** |
| **R05** | `SessionRendered` rendered_text() cumulative | ActivePreviewOnly off toggle | **FALSIFIED** @00:19/00:31/02:13 | runtime-falsified | **STAGE_FAIL (P1)** |
| **I04** | Format/Copy/Agent buttons + selectors; auto_paste off; force_raw | "Save"/"Augment" gone | "formatowanie nie dzieje się automatycznie — to był wymóg" ✓ | STRONG | **STAGE_PASS** |
| **B01** | single `send_assistive_with_agent_runtime` | brak per-utterance dispatch | **podmianka** w Emil, timing niejasny | MEDIUM / runtime-partial | **STAGE_PARTIAL (P1)** |
| **NG01** | diff confined to app/+docs | brak Apple/tail/layered kodu | N/A | STRONG | **STAGE_PASS** |
| **A01** | R3/R4/I04 ✓ | append@UI ✗ | **"sukces połowiczny"** | STRONG | **NOT_ACCEPTED (P0)** |

**Test-gap (kluczowy):** `stop_toggle_and_adjudicate` linia 3056 `if cfg!(test) { return stop_toggle_recording().await; }` + `recorder.stop()` za `if !cfg!(test)`. Realna ścieżka Fazy 1 **nie jest dotykana** przez testy. Istniejące testy (`test_action_contract_mode_*`, `test_should_use_toggle_adjudicated_stop_only_*`, `test_current_segment_text_smoke`) pokrywają routing/labels/accessor — **nie** zachowanie append. To zgodne z CLAUDE.md (CI = tylko `cargo fmt`; recorder/AppKit/Metal nietestowalne w CI) — dlatego runtime operatora pełni rolę testu akceptacyjnego.

## Phase 6 — Stage-Aware Verdict

| Task | Stage audited | Landed scope | Deferred scope | Verdict | Evidence |
|---|---|---|---|---|---|
| faza1 | Faza 1 (continuity append) | backend buffer continuity, WAV retain, action contract, per-utterance removal | Faza 2 (Apple/tail/lexicon/paralingual/BAM) — gated | **STAGE_PARTIAL** | code STRONG + runtime partial; UI append fail |

## Phase 7 — Per-Task Verdict

| Task | Frontmatter | Stage | Req checked | Implemented | Partial | Missing | Contradictions | Neg checks | Tests | Verdict | Severity |
|---|---|---|---|---|---|---|---|---|---|---|---|
| faza1 | PROPOSED | Faza 1 | 9 | 5 | 2 | 0 | 1 (R05 UI replace) | 5/5 pass (code) | routing only; core path uncovered | **STAGE_PARTIAL** | P0 (gate) |

## Phase 8 — Self-Attack + Model Check

**Self-attack na PASS-ach:**
- *R03/R04 oparte na produkcyjnej ścieżce za `cfg!(test)` — czy na pewno działa?* Runtime operatora ("mamy pełne transkrypcje") potwierdza retencję → PASS się broni.
- *I04 PASS — czy [Format] na pewno nie auto-odpala?* Operator wprost: "formatowanie nie dzieje się automatycznie, mogłeś wcisnąć format" → PASS się broni, wzmocniony runtime.
- *Czy R05 to nie zbyt surowy FAIL — może to tylko kosmetyka?* Nie: to **główny user-visible cel Fazy 1** wg ADR § Ból nr 1 + Niezmiennik 3. Operator nazywa go nie-naprawionym. FAIL utrzymany.
- *Czy R01 nie powinien być FAIL skoro UI zawodzi?* Nie — backend continuity realnie wylądował (operator potwierdza buffer); to PARTIAL, nie FAIL. Rozdzielenie warstw jest istotą diagnozy.

**Najsłabszy dowód:** B01 (Emil) — code MEDIUM, runtime tylko obserwacyjny ("de facto ten sam efekt"); operator nie domknął testu wysyłki. Zostawiam PARTIAL, nie PASS.

**Model Check:**
- assumptions: STT screenscribe ma drobne przekłamania (np. "apremindowania"=appendowania, "bagedzie"=bufferze) — sens jednoznaczny z kontekstu, nie zmienia werdyktu.
- obszary niepewne: dokładny mechanizm renderu overlay (Preview vs SessionRendered) wymaga code-trace `app/ui/overlay` ↔ `app/presentation/emitter` — poza scope read-only auditu, to następny ruch naprawczy.
- najtrudniejsze do weryfikacji: B01 timing wysyłki do Emila.
- claims odrzucone bez kodu: commit "Gates ... cargo test --lib OK" (test nie pokrywa ścieżki Fazy 1, więc nie jest dowodem na append).
- staged risk: gdyby sądzić full-plan zamiast landed-stage, werdykt brzmiałby FAIL; landed-stage to STAGE_PARTIAL — uczciwsze.

```
model_confidence: high
```

---

## Cross-finding

| Finding | Affected | Evidence | Severity | Recommendation |
|---|---|---|---|---|
| Layer split: backend appenduje, prezentacja podmienia | R01, R05, B01 | runtime @02:04 | P0/P1 | naprawa w app/ui/overlay + emitter, jeden cut |
| Realna ścieżka stop poza testem | R01,R03,R04,B01 | `cfg!(test)` 3056/2916 | P2 | headless test na rendered_text() cumulative |
| Litera-spec append_mode vs equivalent | R01 | mod.rs:3692 false | P3 | doc note w ADR/kod-komentarz |

---

---

## Rekonsiliacja z operatorską recką (curated, 2026-05-28 15:40)

Operatorska ręczna recka (`..._review/TODO_*.md` + annotated frames) **w pełni zbiega się** z auditem i zaostrza go:

- **#1 CRITICAL** = R05 (append na końcu ogona nie działa). Operator podniósł to z mojego P1 → **P0**.
- **#2/#3/#4/#5/#7** = warianty tego samego: overlay renderuje podmianę / "bruno" / nieuporządkowany tekst → wszystkie pod R05.
- **#9 LOW** = B01 timing wysyłki do Emila.
- **#8** = NOWY, osobny wątek: czy overlay przyjmuje linki. Poza Fazą 1, do backlogu.

**MYLIK w AI-warstwie screenscribe (#6) — NIE działać wg niego.** Auto-generowane "Actions" dla #6 mówią *"włączyć automatyczne formatowanie"* — to sprzeczne z intencją operatora z narracji (*"formatowanie nie dzieje się automatycznie — to też był wymóg"*, I04 = wymóg SPEŁNIONY). To artefakt AI-layer, nie operator intent. Klasyczny mylik (actor/function smear): obserwacja "nie auto-formatuje" przepisana na akcję "włącz auto-format".

## Root-cause diagnosis (read-only trace)

Append-regression NIE leży w oczywistych miejscach:

- `append_transcription_delta_impl` (overlay:1434) — POPRAWNY: `TranscriptDelta::from_raw(delta).apply(&mut accumulated_text)`, kumulatywny backspace-protokół.
- `route_transcription_delta` (helpers:66) — POPRAWNY: woła append path, kontrakt zakazuje full-snapshotów.
- `set_transcription_text` (wholesale replace, overlay:1477) — **0 callerów** w `app/`, hipoteza klobrowania ODPADA.
- emitter `SessionRendered` (emitter:315) → `send_cmd(SetTargetText(rendered_text()))` — POPRAWNY, wysyła skumulowany tekst; `BufferedEmitter::set_target_text` diffuje full→delta.

**Najsilniejsza hipoteza (wymaga instrumentacji runtime):** kadencja `committed` vs `active_preview` w `SessionState`. `apply_preview` (emitter:66) ZASTĘPUJE `active_preview` (utterance-local); tylko `UtteranceFinal`→`finalize()` (emitter:354) przenosi preview→committed. Jeśli w streaming-toggle path `UtteranceFinal` nie odpala podczas ciągłego dyktowania (tylko `Preview`/`Correction`), to `committed` zostaje puste i `rendered_text() = committed + active_preview` pokazuje wyłącznie najświeższy preview → **replace-effect**. Druga możliwość: BufferedEmitter emituje "backspace-all + retype" gdy preview baseline się resetuje. Obie wymagają runtime-trace (instrument `UtteranceFinal` cadence + delta granularity), nie statyki.

**Następny cut (gdy autoryzowany):** instrument emitter (`UtteranceFinal`/`committed` log) + jedna sesja runtime operatora → potwierdzenie, czy `UtteranceFinal` brakuje w streaming. Jeśli tak — fix po stronie STT streaming emission (gdzie engine emituje boundary events) albo wymuszenie periodic-commit w emitterze. Mierzy w `app/presentation/emitter.rs` + ścieżkę emisji eventów STT, NIE w `core` engine.

---

_Audit READ-ONLY: żaden plik kodu nietknięty. Artefakty: `audit_report.md`, `audit_requirements_matrix.jsonl`, `audit_trace.log` + screenscribe pack w `/tmp/faza1-screenscribe` + operatorska recka w `..._review/`._

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_
