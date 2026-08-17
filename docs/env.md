# Codescribe — Konfiguracja środowiska (ENV)

Ten dokument porządkuje **wszystkie zmienne środowiskowe** używane przez codescribe. Układ jest „dla weterynarza”:

1. co jest domyślne,
2. co jest wymagane,
3. jakie są zasady nadpisywania i konflikty,
4. pełny podział tematyczny.

**Legenda zmian:**

- **(HOT RELOADED)** – działa od następnej akcji/nagrania (ale tylko jeśli env jest ustawiony w procesie; ręczna edycja `.env` wymaga restartu).
- **(RESTART NEEDED)** – wymaga restartu aplikacji.
- **(REBUILD NEEDED)** – wymaga przebudowania binarki.

> Źródło prawdy: zmienne używane w kodzie (runtime + build + testy). Nie ma tu żadnych „nowych” envów.
>
> **Uwaga:** GUI zapisuje „regularne” ustawienia do `~/Library/Application Support/Codescribe/settings.json`,
> a sekrety trafiają do **macOS Keychain**. `.env` jest tylko dla power‑userów i override’ów.
>
> **Hotkeys truth:** per-mode bindings żyją już tylko w `settings.json` (`Settings → Modes & Shortcuts`).
> `.env` nie ustawia już `Dictation` / `Formatting` / `Assistive` bindings.

---

## A) Systemowe domyślne (defaults)

Poniższe działają „same z siebie” — jeśli ich nie ustawisz, aplikacja ma sensowne wartości domyślne.

**Hotkeys / UI / zachowanie podstawowe**

- mode bindings (`Dictation`, `Formatting`, `Assistive`) – konfigurowane w `settings.json` przez GUI
- `HOLD_EXCLUSIVE` – domyślnie `0` (RESTART NEEDED) — `1` robi Fn-hold RAW-only i wyłącza arm modifier (domyślnie Shift→Chat; Cmd jako alternatywa w Settings)
- `HOLD_ARM_MODIFIER` – domyślnie `shift` — `cmd` ustawia Command jako modifier armujący assistive (W10-B)
- `HOLD_START_DELAY_MS` – domyślnie `800` (RESTART NEEDED)
- `DOUBLE_TAP_INTERVAL_MS` – domyślnie `200` (RESTART NEEDED)
- `TOGGLE_SILENCE_SEC` – domyślnie `5.0` (HOT — next recording): cisza zamyka epokę Apple (silnik odpoczywa, Layer 1 dostaje zapieczętowany span). Nie `CODESCRIBE_VAD_*` (te nazwy nigdy nie były czytane).
- `SHOW_TRAY_GLYPH` – domyślnie `1` (RESTART NEEDED)
- `HOLD_INDICATOR` – domyślnie `1` (HOT — next badge show; K3 W10-E, no live redraw of visible badge)
- `HOLD_BADGE_SIZE` – domyślnie `8` (HOT — next badge show; K3 W10-E)
- `HOLD_BADGE_OFFSET_X` – domyślnie `10` (RESTART NEEDED)
- `HOLD_BADGE_OFFSET_Y` – domyślnie `-10` (RESTART NEEDED)
- `OVERLAY_POSITION_MODE` – domyślnie `snapped_top_right` (RESTART NEEDED)
- `CODESCRIBE_OVERLAY_STABLE_PREVIEW` – domyślnie `0` (HOT RELOADED; `1` = tylko stabilny preview)

**Dźwięk / feedback**

- `BEEP_ON_START` – domyślnie `1` (RESTART NEEDED)
- `SOUND_NAME` – domyślnie `Tink` (RESTART NEEDED)
- `SOUND_VOLUME` – domyślnie `0.25` (RESTART NEEDED)

**Audio / silence**

- `AUTO_SILENCE` – domyślnie `0` (RESTART NEEDED)

**Historia / storage**

- `HISTORY_ENABLED` – domyślnie `1` (zawsze ON) (RESTART NEEDED)
- `DUMP_AUDIO_LOGS` – domyślnie `1` (zawsze ON) (RESTART NEEDED)

**Streaming (chunky)**

- `CODESCRIBE_STREAM_CHUNK_SEC` – domyślnie `3.0` (HOT RELOADED)
- `CODESCRIBE_STREAM_OVERLAP_RATIO` – domyślnie `0.2` (HOT RELOADED)
- `CODESCRIBE_MAX_INFERENCE_CONCURRENCY` – domyślnie `1` (HOT RELOADED; clamp `1..4`)
- `CODESCRIBE_BUFFER_DELAY_MS` – domyślnie `280` (HOT RELOADED)
- `CODESCRIBE_TYPING_CPS` – domyślnie `90` (HOT RELOADED)
- `CODESCRIBE_EMIT_WORDS_MAX` – max słów na tick (buffered), domyślnie `2` (HOT RELOADED)

**VAD (Silero neural network)**

- brak runtime envów: VAD używa hardcoded defaults z `core/vad/config.rs` (RESTART NEEDED)
- `threshold=0.5`
- `min_speech_duration=0.064s`
- `min_silence_duration=0.0s`
- `speech_pad/pre_roll=0.064s`
- `max_speech_duration=∞`

> **Uwaga:** VAD config jest read-only po inicjalizacji (OnceLock). Zmiana wymaga restartu aplikacji.

**Post‑process (gating)**

- `CODESCRIBE_STREAM_SIMILARITY` – domyślnie z kodu (HOT RELOADED)
- `CODESCRIBE_STREAM_NOVELTY` – domyślnie z kodu (HOT RELOADED)

**Model lokalny (embedded-first)**

- brak wymaganych envów, jeśli build znalazł kompletny model przy kompilacji
- jeśli build powstał z `CODESCRIBE_NO_EMBED=1` albo bez modelu, runtime użyje fallbacku przez `CODESCRIBE_MODEL_PATH`, cache lub skonfigurowane ścieżki

---

## B) Niezbędne do działania (required)

Samo **local‑only** uruchomienie nie wymaga żadnych envów.
Poniżej — kiedy coś staje się wymagane.

**1) Cloud final transcript (gdy nie chcesz commitować local transcriptu)**
Wymagane **tylko jeśli** `USE_LOCAL_STT=0` (RESTART NEEDED):

- `STT_ENDPOINT` (RESTART NEEDED)
- `STT_API_KEY` (RESTART NEEDED)

**2) AI Formatting / Assistive (gdy włączasz AI)**
Wymagane **tylko jeśli** `AI_FORMATTING_ENABLED=1` i chcesz LLM:

- `LLM_ENDPOINT`, `LLM_MODEL`, `LLM_API_KEY` (HOT RELOADED)
  - albo tryb‑specyficzne: `LLM_FORMATTING_*` i/lub `LLM_ASSISTIVE_*` (HOT RELOADED)

**3) Brak lokalnego modelu w ścieżkach runtime**
Wymagane **tylko jeśli** build działa bez embedded Whispera (`CODESCRIBE_NO_EMBED=1` albo brak modelu przy buildzie)
i runtime nie może znaleźć Whispera przez cache / config:

- `CODESCRIBE_MODEL_PATH` (RESTART NEEDED)

---

## C) Override / konflikty / hierarchie (dla człowieka)

**Tu są zasady priorytetów — co nadpisuje co.**

**Ścieżki / data dir**

- `CODESCRIBE_DATA_DIR` > `~/.codescribe` (RESTART NEEDED)
- `CODESCRIBE_ENV_PATH` nadpisuje lokalizację `.env` (RESTART NEEDED)

**Model lokalny**

- `CODESCRIBE_MODEL_PATH` **nadpisuje runtime lookup** (RESTART NEEDED)
- `CODESCRIBE_NO_EMBED=1` (build‑time) wyłącza opcjonalne embedy, w tym Whisper; wtedy `CODESCRIBE_MODEL_PATH` lub HF cache stają się fallbackiem runtime

**STT endpointy**

- `STT_ENDPOINT` (RESTART NEEDED)
- Jeśli ustawisz `STT_ENDPOINT`, **wymagany jest** `STT_API_KEY` (RESTART NEEDED)

**LLM / endpointy**

- `LLM_FORMATTING_*` **nadpisuje** `LLM_*` dla formattingu (HOT RELOADED)
- `LLM_ASSISTIVE_*` **nadpisuje** `LLM_*` dla assistive (HOT RELOADED)

**Streaming runtime**

- App runtime używa jednej ścieżki: `start_event_session` + `transcription_session`.
- `TRANSCRIPT_SEND_MODE=streaming` wysyła delty do overlayu (RESTART NEEDED)

**Overlay pozycja**

- `OVERLAY_POSITION_MODE=custom` aktywuje `OVERLAY_CUSTOM_X/Y` (RESTART NEEDED)

---

## D) Pełny podział działowy (wszystkie zmienne)

### Audio

- `AUDIO_INPUT_DEVICE` – nazwa urządzenia wejściowego (RESTART NEEDED)
- `CODESCRIBE_CAPTURE_LEVEL_LOW_DB` (HOT RELOADED; default `-52`) — próg WARN `capture_level_low` w dBFS, liczony od **mediany RMS mowy aktywnej** (nie od mediany całego nagrania). Cyfrowe zera ciszy na macOS 27 nie mogą zaniżać progu. Receipt nigdy nie jest terminalny.
- `AUTO_SILENCE` (RESTART NEEDED)

### Transkrypcja (local/cloud)

- `CODESCRIBE_INLINE_FORMAT` (HOT RELOADED; default `0`) — uzbraja bufor formatowania chunków W13-1; timeouty `CODESCRIBE_INLINE_FORMAT_{CHUNK,FLUSH,TAIL}_TIMEOUT_MS` mają domyślne budżety odpowiednio 10000/2500/15000 ms. Default pozostaje guzikiem operatora.
- `USE_LOCAL_STT` (RESTART NEEDED)
- `LOCAL_MODEL`, `WHISPER_MODEL` (RESTART NEEDED)
- `WHISPER_LANGUAGE` (HOT RELOADED; default `auto`; applies to the next capture. `auto` leaves language detection to Whisper for mixed-language dictation.)
- `CODESCRIBE_WHISPER_INITIAL_PROMPT` (RESTART NEEDED; alias legacy: `WHISPER_INITIAL_PROMPT`; ignorowane przez ONNX)
- `STT_ENDPOINT`, `STT_API_KEY` (RESTART NEEDED)
- `FINAL_PASS_MODE` (legacy; `always|smart|off`; alias `CODESCRIBE_FINAL_PASS_MODE`) — zachowany do migracji ustawień, ale zwykły stop nie wykonuje już żadnego file-passu. Pełny WAV trafia do STT wyłącznie po jawnej akcji Retranscribe (Overlay/Dictionary/Teacher). Live refinement wybiera osobny `CODESCRIBE_LAYERED_TRANSCRIPTION`; słownik/lexicon zawsze działa w postprocess.
- `CODESCRIBE_LAYERED_TRANSCRIPTION` (HOT RELOADED; default `phase1`; `phase1`..`phase4` lub bare `1`..`4`; jawne `off`/`0`/`false` rozbraja) — ortogonalny gate warstwowej transkrypcji. Unset → `phase1` od 2026-08-09. `phase1+` = Layer 1 Whisper tail-patch (`ReplaceRange`) na **obu** ścieżkach live: VAD/scheduler oraz domyślnym Apple progressive live (W2-A — gap-fill z zatrzymanego PCM, max jeden job w locie, nierozwiązane okno nigdy nie trafia do Whispera). Promoted do `settings.json`. Smart final-pass **nie** ustawia tej flagi.
- `STT_TAIL_PROVIDER` (HOT RELOADED; default `inprocess`) — wybiera implementację kontraktu tail-patch: `inprocess`, nadzorowany lokalny `sidecar` po WebSocket albo `remote` po multipart. Awaria sidecara/remote przechodzi do in-process z typed receiptem; zmiana defaultu pozostaje guzikiem operatora.
- `CODESCRIBE_STT_SIDECAR_BIN` (RESTART NEEDED; dev only) — jawna ścieżka do helpera; aplikacja dystrybucyjna automatycznie znajduje `codescribe-stt-sidecar` obok własnego executable.
- `CODESCRIBE_TAIL_PATCH_MAX_CHANGE_RATIO` (HOT RELOADED; default `0.5`) — próg bezpieczeństwa Layer 1: jeśli udział zmienionych znaków wobec zatwierdzonej wypowiedzi przekracza tę wartość, cała łatka jest **odrzucana** zamiast nałożona. Dzięki temu rozbieżna re-transkrypcja nigdy nie nadpisze żywego płótna.
- `CODESCRIBE_TAIL_PATCH_SMALL_EDIT_FLOOR` (HOT RELOADED; default `3`) — budżet małych poprawek Layer 1: łatka o kształcie substytucji dotykająca co najwyżej tylu tokenów omija próg `MAX_CHANGE_RATIO`. Sam próg względny strukturalnie głodzi krótkie wypowiedzi (każda realna poprawka 1 słowa na 1-3-tokenowym commicie to ≥50% zmiany — zmierzone 2026-08-12: 116 skipów, 0 nałożonych). Czyste insercje nigdy nie korzystają z tego budżetu.
- `CODESCRIBE_APPLE_DICTATION_TRANSCRIBER` (RESTART NEEDED; default `0`) — uzbraja ścieżkę PoC `DictationTranscriber` (W4-A): moduł SpeechAnalyzer stojący za SYSTEMOWYM dyktowaniem, jedyny analizator Apple, którego katalog zawiera pl-PL. Domyślnie **wyłączone** — nieuzbrojony rung nie istnieje, a kolejność backendów jest bajt w bajt taka jak w wersji wydanej. Klucz czytają obie strony (proces Rust i dziedziczący go bridge), więc jeden wpis uzbraja całą ścieżkę. **To nie jest domyślna ścieżka produktu** — przełączenie jest decyzją operatora.
- `CODESCRIBE_MODEL_PATH`, `CODESCRIBE_MODELS_DIR` (RESTART NEEDED)
- `CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS` (HOT RELOADED dla wartości progu; default `1800` = 30 min; `0` jest jawnym keep-warm — włączenie reaper'a z `0` wymaga restartu) — po N s od **skończonego** dekodowania silnik Whisper jest zwalniany z pamięci (GPU/host) i ładowany ponownie przy następnym użyciu. Bezczynność nie jest stanem UI i nie zaczyna się w momencie acquire — długi Retranscribe/HQ nie wygląda jak idle. Każdy cold-load loguje `whisper_residency_load`, a reaper `whisper_residency_reclaim`: zdarzenia INFO mają efektywny TTL, liczniki oraz czasy load/unload/reclaim, lecz nigdy audio ani treść transkrypcji. Pełny fp16 model jest ścieżką produktu, więc następny cold-load nie płaci już każdorazowej dekwantyzacji q8→F32. `0` kupuje zerową latencję kosztem trzymanego RSS.

### Streaming / VAD / buffer

- `CODESCRIBE_STREAM_CHUNK_SEC` (HOT RELOADED)
- `CODESCRIBE_STREAM_OVERLAP_RATIO` (HOT RELOADED)
- `CODESCRIBE_MAX_INFERENCE_CONCURRENCY` (HOT RELOADED; clamp `1..4`)
- `CODESCRIBE_BUFFER_DELAY_MS` (HOT RELOADED)
- `CODESCRIBE_TYPING_CPS` (HOT RELOADED)
- VAD internals: hardcoded (no env knobs)

### Post‑process (gating / embeddings)

- `CODESCRIBE_STREAM_SIMILARITY` (HOT RELOADED)
- `CODESCRIBE_STREAM_NOVELTY` (HOT RELOADED)
- `CODESCRIBE_STREAM_DISABLE_EMBEDDINGS` (HOT RELOADED)
- `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS` (HOT RELOADED dla wartości progu; default `2700` = 45 min; `0` wyłącza — włączenie z `0` wymaga restartu) — po N s bezczynności embedder MiniLM jest zwalniany z pamięci (GPU/host) i ładowany ponownie przy następnym użyciu (zmierzony zimny start: ~0,9 s)
- `CODESCRIBE_EMBEDDER_DEVICE` (RESTART NEEDED; `cpu` wymusza CPU, inaczej Metal z fallbackiem) — przełącznik diagnostyczny. Powstał, bo backend potrafi zwracać wartości nie-liczbowe po cichu: na macOS 27 / Metal ten model dawał 384 wymiary NaN dla każdego wejścia w bf16, a te same wagi w f32 liczyły poprawnie. Ładowanie ma teraz samotest skończoności i samo degraduje proces do CPU, gdy backend zwróci NaN

### LLM (formatting/assistive)

- `AI_FORMATTING_ENABLED` (RESTART NEEDED)
- `LLM_ENDPOINT`, `LLM_MODEL`, `LLM_API_KEY` (HOT RELOADED)
- `LLM_FORMATTING_ENDPOINT`, `LLM_FORMATTING_MODEL`, `LLM_FORMATTING_API_KEY` (HOT RELOADED)
- `LLM_ASSISTIVE_ENDPOINT`, `LLM_ASSISTIVE_MODEL`, `LLM_ASSISTIVE_API_KEY` (HOT RELOADED)
- `LLM_TEMPERATURE`, `LLM_FORMATTING_TEMPERATURE`, `LLM_ASSISTIVE_TEMPERATURE` (HOT RELOADED)
- `LLM_USE_STREAMING` (HOT RELOADED)
- `AI_MAX_TOKENS`, `AI_ASSISTIVE_MAX_TOKENS` (RESTART NEEDED)
- `TRANSCRIPT_SEND_MODE` (RESTART NEEDED)
- `CODESCRIBE_AI_MAX_RETRIES`, `CODESCRIBE_AI_RETRY_DELAY_MS`, `CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS`, `CODESCRIBE_AI_OLLAMA_ATTEMPT_TIMEOUT_MS` (HOT RELOADED)
- `CODESCRIBE_RESPONSES_PROBE_URL` (HOT RELOADED; default pusty = oficjalny endpoint providera) — opcjonalny cel sondy *zdolności* Responses (puste body ⇒ 400/401). Nie bramkuje sign-in: OAuth zapisuje tożsamość wiersza; zapis do Responses to osobny lane/klucz.

### Hotkeys

- per-mode bindings w `settings.json` (`Settings → Modes & Shortcuts`)
- `HOLD_EXCLUSIVE` (RESTART NEEDED)
- `HOLD_START_DELAY_MS` (RESTART NEEDED)
- `DOUBLE_TAP_INTERVAL_MS` (RESTART NEEDED)
- `TOGGLE_SILENCE_SEC` (HOT — next recording; Apple epoch lifecycle, not wav-VAD auto-send)

### UI / Overlay / Feedback

- `SHOW_TRAY_GLYPH` (RESTART NEEDED)
- `HOLD_INDICATOR` (HOT — persisted immediately; next badge show, no live redraw)
- `HOLD_BADGE_SIZE` (HOT — persisted immediately; next badge show, no live redraw)
- `HOLD_BADGE_OFFSET_X` (RESTART NEEDED)
- `HOLD_BADGE_OFFSET_Y` (RESTART NEEDED)
- `OVERLAY_POSITION_MODE`, `OVERLAY_CUSTOM_X`, `OVERLAY_CUSTOM_Y` (RESTART NEEDED)
- `BEEP_ON_START`, `SOUND_NAME`, `SOUND_VOLUME` (RESTART NEEDED)

### Clipboard

- `RESTORE_CLIPBOARD` (RESTART NEEDED)
- `RESTORE_CLIPBOARD_DELAY_MS` (RESTART NEEDED)

### Storage / History

- `HISTORY_ENABLED` (RESTART NEEDED)
- `DUMP_AUDIO_LOGS` (RESTART NEEDED)
- `CODESCRIBE_DATA_DIR`, `CODESCRIBE_ENV_PATH` (RESTART NEEDED)

### Quality / Report

- `QUALITY_DEBUG_MODE` (HOT RELOADED)
- `CODESCRIBE_QUALITY_DISABLE_CLOUD` (HOT RELOADED)

### Backend / misc

- `CODESCRIBE_BACKEND_URL` (HOT RELOADED)
- `BACKEND_MAX_UPLOAD_MB` (RESTART NEEDED)

### Debug / test only

- `CODESCRIBE_DEBUG_TOKENS` (RESTART NEEDED)
- `CODESCRIBE_STREAM_FORCE_EMBEDDINGS` (RESTART NEEDED)
- `CODESCRIBE_E2E_STT` (RESTART NEEDED)
- `CODESCRIBE_E2E_DAEMON` (RESTART NEEDED)
- `CODESCRIBE_E2E_MIC` (RESTART NEEDED)
- `CODESCRIBE_E2E_MIC_LANGUAGE` (RESTART NEEDED)
- `CODESCRIBE_E2E_LANG` (RESTART NEEDED)

---

## Testy (dla człowieka, bez bólu)

Makefile **automatycznie** ładuje `~/.codescribe/.env` przy uruchamianiu testów.

**Domyślnie** testy używają Twojej referencji z `~/.codescribe/.env`.
Jeśli chcesz wymusić lokalny test LLM (diagnostycznie), uruchom:

```
TEST_USE_LOCAL_LLM=1 make test
```

Domyślny endpoint aplikacji:

```
https://api.openai.com/v1/responses
formatting model: gpt-4.1
assistive model: gpt-5.5
```

Jeśli chcesz tylko SSE / streaming:

```
make test-sse
```

- `CODESCRIBE_E2E_OLLAMA` (RESTART NEEDED)
- `CODESCRIBE_E2E_RUN_MEDIUM` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS_DATE` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS_LIMIT` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS_MAX_REGRESSION` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS_LANGUAGE` (RESTART NEEDED)
- `CODESCRIBE_E2E_CORPUS_DIR` (RESTART NEEDED)

### Logging (legacy)

- `LOG_LEVEL` (legacy) (RESTART NEEDED)
- `RUST_LOG` (RESTART NEEDED)

---

# Minimal config (działające przykłady)

**1) Local only (embedded, bez LLM)**

```
USE_LOCAL_STT=1
# (optional) język/hint; auto is the default for mixed-language dictation:
# WHISPER_LANGUAGE=auto
```

**2) Local + AI formatting**

```
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1
# Store LLM_API_KEY in Settings / macOS Keychain.
```

**3) Cloud‑only STT**

```
USE_LOCAL_STT=0
STT_ENDPOINT=https://api.libraxis.cloud/v1/audio/transcriptions
STT_API_KEY=...
```

**4) Strojenie streaming / powtórki**

```
CODESCRIBE_STREAM_CHUNK_SEC=12
CODESCRIBE_STREAM_SIMILARITY=0.90
CODESCRIBE_STREAM_NOVELTY=0.20
```

---

## Report expectation (dla wdrożeniowca)

- Jeśli coś nie działa: sprawdź `~/.codescribe/.env` i porównaj z sekcją „B” i „C”.
- Jeśli pojawiają się powtórki: zwiększ `CODESCRIBE_STREAM_CHUNK_SEC`.
- Jeśli brak AI: sprawdź czy `LLM_*` są ustawione.
