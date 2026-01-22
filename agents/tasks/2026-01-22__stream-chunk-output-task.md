Zadanie: domknąć prawdziwy streaming transkrypcji do CLI w CodeScribe.

Kontekst:
- Streaming/chunking istnieje w core (callbacki, streaming_recorder, transcribe_long_streaming).
- CLI ma flagę --stream, ale finalnie robi tylko println(final_text) i nie streamuje chunków.
- Brak jawnej komendy "transcribe live" (mic → stdout).

Wymagania:
1) codescribe transcribe --stream <file> ma emitować delty/chunki do stdout w trakcie, z flush.
2) Dodać subkomendę: codescribe transcribe live (mic → stdout) opartą o StreamingRecorder + StreamDeltaCallback.
3) Minimalna mikro-abstrakcja outputu (jedno miejsce emitowania), bez refactoru świata.
4) Testy: e2e dla --stream (fixture wav), live jako manual smoke test lub test “offline streaming”.

Definition of Done:
- --stream daje wieloczęściowy output (nie jedna linia na końcu).
- --stream flushuje, więc widać postęp na żywo.
- "transcribe live" działa ręcznie i pisze na stdout.
- Testy przechodzą.

Deliverables:
- PR z kompletnymi zmianami + opis jak uruchomić (komendy). + nie naruszanie pipeline codescribe-quality i codescribe-loop

##############################################
#       APPENDIX - inventory			           #
# Stream/Chunk/Output Inventory (CodeScribe) #
##############################################

**Scope:** Inwentaryzacja miejsc w kodzie powiązanych z transkrypcją, streamem, chunkowaniem i outputem do terminala. Źródło: tylko `loct`.

## 1) Komendy CLI (entrypointy, flagi)
- `src/main.rs` — główny CLI (`clap::Parser` + `Commands::Transcribe { ... }`). Zawiera `handle_transcribe_command(...)` oraz flagę `stream: bool` dla transkrypcji z pliku. Output do stdout jest w tym pliku (finalny `println!`).
- `src/bin/codescribe_quality.rs` — osobny bin do quality reportów.
- `src/bin/codescribe_loop.rs` — osobny bin do quality loopów.
- `tests/e2e_cli_commands.rs` — testy CLI (`codescribe`, `codescribe transcribe --help`, `codescribe transcribe <file>`).

## 2) Pipeline transkrypcji (wejście → przetwarzanie → wynik)
- **Wejście audio (mic):** `codescribe-core/src/audio/recorder.rs` — Recorder + VAD + zapis WAV.
- **Streaming z mic / chunkowanie:** `codescribe-core/src/audio/streaming_recorder.rs` — zbiera próbki, tnie na chunki, transkrybuje i zwraca delty przez callback.
- **Whisper lokalnie:**
  - `codescribe-core/src/whisper/engine.rs` — `transcribe_*`, w tym `transcribe_long_streaming`.
  - `codescribe-core/src/whisper/singleton.rs` — singleton modelu + `transcribe_streaming`.
- **Post-process streamu:** `codescribe-core/src/stream_postprocess.rs` — lexicon + gate + cleanup.
- **Cloud STT / streaming:** `src/libraxis.rs` — WebSocket + NDJSON streaming (partial + final).
- **Kontroler aplikacji (tray/mode):** `src/controller/mod.rs` — start/stop nagrywania, ustawianie callbacków, routing delty do overlaya.
- **UI konsument delty:** `src/transcription_overlay.rs` — `append_transcription_delta` (aktualizacja live textu).

## 3) Streaming / chunking (miejsca i rola)
- `codescribe-core/src/audio/streaming_recorder.rs`
  - `StreamDeltaCallback` + `set_delta_callback` → live preview.
  - chunkowanie wg `stream_chunk_duration_sec()` + `stream_overlap_sec()`.
  - `transcribe_streaming_samples(...)` — streaming „offline” po próbkach.
- `codescribe-core/src/whisper/engine.rs`
  - `transcribe_long_streaming(...)` + `ChunkCallback`.
- `codescribe-core/src/ai_formatting.rs`
  - SSE streaming dla LLM (`AiStreamCallback`).
- `src/libraxis.rs`
  - WebSocket streaming (`transcribe_websocket`) + NDJSON streaming (`transcribe_ndjson`).
- `tests/e2e_streaming_chunks.rs`, `tests/e2e_sse_streaming.rs` — testy streamu i chunków.

## 4) Output do terminala (stdout/stderr/flush)
- `src/main.rs` — output transkrypcji do stdout (`println!`) + progres na stderr (`eprintln!`).
- `src/bin/codescribe_quality.rs`, `src/bin/codescribe_loop.rs` — raporty/loop output przez `println!/eprintln!`.
- `src/backend.rs` — czytanie `stdout/stderr` z procesów.
- `src/ipc/server.rs` — `writer.flush().await?`.
- `examples/*` — demonstracje i testy wypisują na stdout.
- `codescribe-core/build.rs` — komunikaty build-time (println/eprintln).

## 5) Luki / ryzyka (na bazie `loct`)
- W `src/main.rs` jest flaga `stream: bool`, ale **CLI transcribe obecnie kończy się pojedynczym `println!` final_text** (brak strumieniowania do stdout po chunkach).
- Nie znaleziono subkomendy `live` w `src/main.rs` (brak jawnego entrypointu do „mic → stdout”).
- Output jest rozproszony po wielu plikach (brak zidentyfikowanego „sink/output writer” jako osobnej abstrakcji).

## Rekomendacja najmniejszej zmiany (po akceptacji)
- **`codescribe transcribe --stream` (plik → stream):** wykorzystać `whisper::transcribe_long_streaming` lub `transcribe_streaming_samples` i emitować delty/chunki na stdout (z flush po każdym chunku).
- **`codescribe transcribe live` (mic → stdout):** użyć `StreamingRecorder` + `StreamDeltaCallback` i wysyłać delty do stdout (analogicznie do overlay), z opcją zakończenia VAD/timeout.
