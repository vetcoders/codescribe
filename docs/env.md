# CodeScribe — Konfiguracja środowiska (ENV)

Ten dokument porządkuje **wszystkie zmienne środowiskowe** używane przez CodeScribe. Układ jest „dla weterynarza”:
1) co jest domyślne,
2) co jest wymagane,
3) jakie są zasady nadpisywania i konflikty,
4) pełny podział tematyczny.

**Legenda zmian:**
- **(HOT RELOADED)** – działa od następnej akcji/nagrania (ale tylko jeśli env jest ustawiony w procesie; ręczna edycja `.env` wymaga restartu).
- **(RESTART NEEDED)** – wymaga restartu aplikacji.
- **(REBUILD NEEDED)** – wymaga przebudowania binarki.

> Źródło prawdy: zmienne używane w kodzie (runtime + build + testy). Nie ma tu żadnych „nowych” envów.

---

## A) Systemowe domyślne (defaults)
Poniższe działają „same z siebie” — jeśli ich nie ustawisz, aplikacja ma sensowne wartości domyślne.

**Hotkeys / UI / zachowanie podstawowe**
- `HOLD_MODS` – domyślnie `ctrl` (RESTART NEEDED)
- `HOLD_EXCLUSIVE` – domyślnie `1` (RESTART NEEDED)
- `TOGGLE_TRIGGER` – domyślnie `double_option` (RESTART NEEDED)
- `HOLD_START_DELAY_MS` – domyślnie `800` (RESTART NEEDED)
- `SHOW_TRAY_GLYPH` – domyślnie `1` (RESTART NEEDED)
- `HOLD_INDICATOR` – domyślnie `1` (RESTART NEEDED)
- `HOLD_BADGE_SIZE` – domyślnie `12` (RESTART NEEDED)
- `HOLD_BADGE_OFFSET_X` – domyślnie `10` (RESTART NEEDED)
- `HOLD_BADGE_OFFSET_Y` – domyślnie `-10` (RESTART NEEDED)
- `OVERLAY_POSITION_MODE` – domyślnie `snapped_top_right` (RESTART NEEDED)

**Dźwięk / feedback**
- `BEEP_ON_START` – domyślnie `1` (RESTART NEEDED)
- `SOUND_NAME` – domyślnie `Tink` (RESTART NEEDED)
- `SOUND_VOLUME` – domyślnie `1.0` (RESTART NEEDED)

**VAD (Voice Activity Detection)**
- `CODESCRIBE_VAD_ENABLED` – domyślnie `1` (RESTART NEEDED)

**Historia / storage**
- `HISTORY_ENABLED` – domyślnie `1` (zawsze ON) (RESTART NEEDED)
- `DUMP_AUDIO_LOGS` – domyślnie `1` (zawsze ON) (RESTART NEEDED)

**Streaming (chunky)**
- `CODESCRIBE_STREAM_CHUNK_SEC` – domyślnie `15.0` (HOT RELOADED)
- `CODESCRIBE_STREAM_OVERLAP_RATIO` – domyślnie `0.25` (HOT RELOADED)
- `CODESCRIBE_BUFFERED_STREAM` – domyślnie `0` (HOT RELOADED)
- `CODESCRIBE_BUFFER_DELAY_MS` – domyślnie `3000` (HOT RELOADED)
- `CODESCRIBE_TYPING_CPS` – domyślnie `30` (HOT RELOADED)
- `CODESCRIBE_EMIT_WORDS_MAX` – max słów na tick (buffered), domyślnie `3` (HOT RELOADED)

**VAD (Silero neural network)**
- `CODESCRIBE_VAD_ENABLED` – włącz/wyłącz auto-stop, domyślnie `1` (RESTART NEEDED)
- `CODESCRIBE_VAD_THRESHOLD` – próg detekcji mowy 0.0-1.0, domyślnie `0.35` (RESTART NEEDED)
- `CODESCRIBE_VAD_SILENCE_SEC` – cisza przed auto-stop, domyślnie `2.5s` (RESTART NEEDED)
- `CODESCRIBE_VAD_MIN_SPEECH_SEC` – min. czas mowy przed detekcją, domyślnie `0.1` (RESTART NEEDED)
- `CODESCRIBE_VAD_MAX_UTTERANCE_SEC` – max. czas wypowiedzi, domyślnie `60` (RESTART NEEDED)
- `CODESCRIBE_VAD_PRE_ROLL_SEC` – pre-roll w sekundach, domyślnie `0.5` (RESTART NEEDED)

> **Uwaga:** VAD config jest read-only po inicjalizacji (OnceLock). Zmiana wymaga restartu aplikacji.

**Post‑process (gating)**
- `CODESCRIBE_STREAM_SIMILARITY` – domyślnie z kodu (HOT RELOADED)
- `CODESCRIBE_STREAM_NOVELTY` – domyślnie z kodu (HOT RELOADED)

**Model lokalny (embedded)**
- brak wymaganych env – model jest w binarce.

---

## B) Niezbędne do działania (required)
Samo **local‑only** uruchomienie nie wymaga żadnych envów.
Poniżej — kiedy coś staje się wymagane.

**1) Cloud STT (gdy wyłączasz local)**
Wymagane **tylko jeśli** `USE_LOCAL_STT=0` (RESTART NEEDED):
- `STT_ENDPOINT` (RESTART NEEDED)
- `STT_API_KEY` (RESTART NEEDED)

**2) AI Formatting / Assistive (gdy włączasz AI)**
Wymagane **tylko jeśli** `AI_FORMATTING_ENABLED=1` i chcesz LLM:
- `LLM_ENDPOINT`, `LLM_MODEL`, `LLM_API_KEY` (HOT RELOADED)
  - albo tryb‑specyficzne: `LLM_FORMATTING_*` i/lub `LLM_ASSISTIVE_*` (HOT RELOADED)

**3) Brak embedded modelu (build dev)**
Wymagane **tylko jeśli** zbudowałeś bez embedu:
- `CODESCRIBE_MODEL_PATH` (RESTART NEEDED)

---

## C) Override / konflikty / hierarchie (dla człowieka)
**Tu są zasady priorytetów — co nadpisuje co.**

**Ścieżki / data dir**
- `CODESCRIBE_DATA_DIR` > `CODESCRIBE_APP_DIR` > `~/.codescribe` (RESTART NEEDED)
- `CODESCRIBE_ENV_PATH` nadpisuje lokalizację `.env` (RESTART NEEDED)
- `CODESCRIBE_ENV_TEMPLATE` tylko do wygenerowania `.env` (jeśli nie istnieje) (RESTART NEEDED)

**Model lokalny**
- `CODESCRIBE_MODEL_PATH` **nadpisuje embedded** (RESTART NEEDED)
- `CODESCRIBE_NO_EMBED=1` (build‑time) → **musisz** ustawić `CODESCRIBE_MODEL_PATH` (REBUILD NEEDED)

**STT endpointy**
- `STT_ENDPOINT` (RESTART NEEDED)
- Jeśli ustawisz `STT_ENDPOINT`, **wymagany jest** `STT_API_KEY` (RESTART NEEDED)

**LLM / endpointy**
- `LLM_FORMATTING_*` **nadpisuje** `LLM_*` dla formattingu (HOT RELOADED)
- `LLM_ASSISTIVE_*` **nadpisuje** `LLM_*` dla assistive (HOT RELOADED)

**Streaming vs buffered**
- `LLM_USE_STREAMING` – streaming odpowiedzi z LLM, domyślnie `1` (HOT RELOADED)
  - `1` lub `true` = SSE streaming (domyślne, pokazuje tekst w czasie rzeczywistym)
  - `0` lub `false` = buffered (czeka na pełną odpowiedź)
- `TRANSCRIPT_SEND_MODE` – tryb wysyłania transkrypcji do UI (RESTART NEEDED)
  - `streaming` = delty w czasie rzeczywistym
  - `buffered` = pełna transkrypcja na końcu
- `CODESCRIBE_BUFFERED_STREAM=1` → **ignoruje** chunking (`CODESCRIBE_STREAM_CHUNK_SEC`) (HOT RELOADED)

**Overlay pozycja**
- `OVERLAY_POSITION_MODE=custom` aktywuje `OVERLAY_CUSTOM_X/Y` (RESTART NEEDED)

---

## D) Pełny podział działowy (wszystkie zmienne)

### Audio
- `AUDIO_INPUT_DEVICE` – nazwa urządzenia wejściowego (RESTART NEEDED)
- `CODESCRIBE_VAD_ENABLED`, `CODESCRIBE_VAD_THRESHOLD`, `CODESCRIBE_VAD_SILENCE_SEC` (RESTART NEEDED)

### Transkrypcja (local/cloud)
- `USE_LOCAL_STT` (RESTART NEEDED)
- `LOCAL_MODEL`, `WHISPER_MODEL`, `WHISPER_LANGUAGE` (RESTART NEEDED)
- `STT_ENDPOINT`, `STT_API_KEY` (RESTART NEEDED)
- `CODESCRIBE_MODEL_PATH`, `CODESCRIBE_MODELS_DIR` (RESTART NEEDED)

### Streaming / VAD / buffer
- `CODESCRIBE_STREAM_CHUNK_SEC` (HOT RELOADED)
- `CODESCRIBE_STREAM_OVERLAP_RATIO` (HOT RELOADED)
- `CODESCRIBE_BUFFERED_STREAM` (HOT RELOADED)
- `CODESCRIBE_BUFFER_DELAY_MS` (HOT RELOADED)
- `CODESCRIBE_TYPING_CPS` (HOT RELOADED)
- `CODESCRIBE_VAD_ENABLED` (RESTART NEEDED)
- `CODESCRIBE_VAD_THRESHOLD` (RESTART NEEDED)
- `CODESCRIBE_VAD_SILENCE_SEC` (RESTART NEEDED)
- `CODESCRIBE_VAD_MIN_SPEECH_SEC` (RESTART NEEDED)
- `CODESCRIBE_VAD_MAX_UTTERANCE_SEC` (RESTART NEEDED)
- `CODESCRIBE_VAD_PRE_ROLL_SEC` (RESTART NEEDED)

### Post‑process (gating / embeddings)
- `CODESCRIBE_STREAM_SIMILARITY` (HOT RELOADED)
- `CODESCRIBE_STREAM_NOVELTY` (HOT RELOADED)
- `CODESCRIBE_STREAM_DISABLE_EMBEDDINGS` (HOT RELOADED)

### LLM (formatting/assistive)
- `AI_FORMATTING_ENABLED` (RESTART NEEDED)
- `LLM_ENDPOINT`, `LLM_MODEL`, `LLM_API_KEY` (HOT RELOADED)
- `LLM_FORMATTING_ENDPOINT`, `LLM_FORMATTING_MODEL`, `LLM_FORMATTING_API_KEY` (HOT RELOADED)
- `LLM_ASSISTIVE_ENDPOINT`, `LLM_ASSISTIVE_MODEL`, `LLM_ASSISTIVE_API_KEY` (HOT RELOADED)
- `LLM_TEMPERATURE`, `LLM_FORMATTING_TEMPERATURE`, `LLM_ASSISTIVE_TEMPERATURE` (HOT RELOADED)
- `LLM_USE_STREAMING` (HOT RELOADED)
- `AI_MAX_TOKENS`, `AI_ASSISTIVE_MAX_TOKENS` (RESTART NEEDED)
- `TRANSCRIPT_SEND_MODE` (RESTART NEEDED)
- `CODESCRIBE_AI_MAX_RETRIES`, `CODESCRIBE_AI_RETRY_DELAY_MS`, `CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS` (HOT RELOADED)

### Hotkeys
- `HOLD_MODS` (RESTART NEEDED)
- `HOLD_EXCLUSIVE` (RESTART NEEDED)
- `TOGGLE_TRIGGER` (RESTART NEEDED)
- `HOLD_START_DELAY_MS` (RESTART NEEDED)

### UI / Overlay / Feedback
- `SHOW_TRAY_GLYPH` (RESTART NEEDED)
- `HOLD_INDICATOR` (RESTART NEEDED)
- `HOLD_BADGE_SIZE` (RESTART NEEDED)
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
- `CODESCRIBE_DATA_DIR`, `CODESCRIBE_APP_DIR`, `CODESCRIBE_ENV_PATH`, `CODESCRIBE_ENV_TEMPLATE` (RESTART NEEDED)

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
Jeśli chcesz **lokalnie** (Ollama), uruchom:
```
TEST_USE_LOCAL_LLM=1 make test
```

Lokalny endpoint (Ollama):
```
http://localhost:11434/v1/responses
model: gpt-oss:120b-cloud
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
# (optional) język:
# WHISPER_LANGUAGE=pl
```

**2) Local + AI formatting**
```
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-...
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
