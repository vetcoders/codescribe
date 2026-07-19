# Analiza Wykonalności: Libraxis Qube Protocol

Dokument ten zawiera analizę obecnego stanu kodu (`Codescribe`) w kontekście wdrożenia nowej architektury **Libraxis Qube** (Centralny Orchestrator + Streaming).

Data: 2026-01-19
Autor: Junie (AI)

> **Historical snapshot.** Module paths reference the pre-refactor tree
> (`src/voice_chat_ui.rs`, `src/controller.rs`). Current truth:
> `app/ui/voice_chat/`, `app/controller/`. Use this ADR for intent, not for file paths.

---

## 1. Stan Obecny (AS-IS)

Analiza kodu wykazała, że obecna architektura jest **monolityczna i zorientowana lokalnie**, choć posiada "uśpione" komponenty gotowe do wykorzystania w nowej wizji.

### Co już mamy ("Nitki"):

1.  **Protokół Strumieniowy (Client-Side)**:

    - W bieżącym kodzie **nie ma** klienta WebSocket – moduł został usunięty podczas porządków.
    - Jeśli Qube wróci, trzeba zdefiniować protokół od zera (ClientMessage/ServerMessage) i dodać klienta.

2.  **Streaming Audio & Whisper**:

    - `src/audio/streaming_recorder.rs` i `src/whisper/` (Singleton/Engine) realizują bardzo wydajne, lokalne przetwarzanie audio (1x RT).
    - Mechanizm ten działa jednak "sztywno" wewnątrz procesu – audio z `cpal` trafia bezpośrednio do `Whisper Engine`.

3.  **UI / Overlay**:

    - `src/voice_chat_ui.rs` to gotowy komponent wizualny (overlay), który może wyświetlać strumieniowane odpowiedzi. Obecnie jest sterowany lokalnie przez `controller.rs`.

4.  **IPC (Unix Socket)**:
    - `src/ipc/server.rs` istnieje, ale obsługuje tylko proste komendy sterujące (Start/Stop/Config). Nie nadaje się do streamingu audio (brak wydajności/protokołu).

### Czego brakuje (Luki):

1.  **Brak Implementacji Serwera (Libraxis Qube Node)**:

    - W projekcie **nie ma kodu serwera WebSocket**. `src/voice_chat.rs` to tylko klient.
    - Brak punktu wejścia (np. `warp`, `axum` lub surowy `tokio-tungstenite`), który mógłby przyjąć połączenie od klienta (lokalnego lub zdalnego).

2.  **Sztywne Sprzężenie w `controller.rs`**:

    - `RecordingController` zarządza `StreamingRecorder`, który "na sztywno" wiąże mikrofon (`cpal`) z lokalnym Whisperem.
    - Brak abstrakcji **Input Source** (Mikrofon vs Strumień Sieciowy) oraz **Processing Backend** (Lokalny Whisper vs Zdalny Libraxis Qube).

3.  **Brak Obsługi Tagów/Demux**:
    - Obecny protokół `VoiceChatEvent` w `src/voice_chat.rs` jest prosty (Transcript/LlmDelta). Nie ma mechanizmu "Tagowania" (np. `<pdf>`, `<tts>`) ani routingu tych tagów do osobnych handlerów.

---

## 2. Analiza Ryzyk i Ograniczeń

1.  **Wydajność (RTF 1x)**:

    - **Ryzyko**: Przesyłanie audio przez WebSocket (nawet lokalnie) dodaje narzut (latency).
    - **Mitygacja**: Whisper v3 Turbo jest bardzo szybki. Opóźnienie sieciowe na `localhost` jest pomijalne (<1ms), a w sieci LAN/WAN akceptowalne przy dobrym łączu. Kluczowe jest użycie binarnego formatu audio (PCM f32), co już jest w `ClientMessage::Chunk`.

2.  **Stabilność Streamingu**:

    - **Ryzyko**: Zerwanie połączenia WS w trakcie dyktowania.
    - **Mitygacja**: Potrzebny mechanizm buforowania lokalnego (w `controller.rs`) i retransmisji, lub po prostu "fail-fast" z informacją dla użytkownika.

3.  **Zależności**:
    - Dodanie serwera WebSocket (np. `axum` lub `tokio-tungstenite` server) zwiększy rozmiar binarki, ale jest niezbędne.

---

## 3. Plan Wdrożenia (Roadmap)

Proponuję implementację w 3 fazach, aby nie zepsuć obecnej funkcjonalności ("Game Changer").

### Faza 1: Libraxis Qube Server (Skeleton)

**Cel**: Uruchomienie serwera WS, który przyjmuje audio i (na razie) tylko loguje/odbija dane (echo).

1.  Dodać zależność `axum` (lub użyć `tokio-tungstenite` w trybie server).
2.  Utworzyć moduł `src/Libraxis Qube/server.rs`.
3.  Zaimplementować obsługę `ClientMessage` po stronie serwera.

### Faza 2: Decoupling Controllera (Abstrakcja)

**Cel**: `controller.rs` może wysyłać audio do "Sink" (Local Whisper lub WS Client).

1.  Wydzielić `AudioSource` z `StreamingRecorder`.
2.  Wprowadzić trait `TranscriptionBackend`:
    - `LocalBackend` (obecny kod: cpal -> whisper).
    - `NetworkBackend` (nowy kod: cpal -> voice_chat_client -> WS).
3.  Umożliwić przełączanie backendu w Configu (Local vs Remote).

### Faza 3: Pełny Libraxis Qube Protocol (Tagi + Routing)

**Cel**: Obsługa wielu kanałów w jednym strumieniu.

1.  Rozszerzyć `ServerMessage` o wariant `TagStart { name: String, metadata: Json }` i `TagEnd`.
2.  Wdrożyć logikę Demuxera w `voice_chat.rs` (klient) – gdy przychodzi tag `<tts>`, kieruj audio do głośników; gdy `<pdf>`, zapisuj plik.
3.  Podłączyć LLM/Agenta po stronie Serwera (Libraxis Qube).

### Faza 4: Unifikacja (Deployment Neutrality)

**Cel**: `codescribe` działa zawsze jako Klient + Server.

1.  Nawet w trybie "lokalnym", aplikacja uruchamia wewnętrzny serwer Libraxis Qube (na `localhost`) i łączy się do niego.
2.  Eliminuje to podział kodu na ścieżki "Local vs Remote" – różnica jest tylko w adresie IP (127.0.0.1 vs Dragon IP).

---

## Rekomendacja

Zalecam rozpoczęcie od **Fazy 1 i 2 równolegle**:

- Stworzenie prostego serwera WS (jako osobna binarka lub flaga `--server`).
- Refaktor `StreamingRecorder` żeby mógł "wypluwać" audio chunks na zewnątrz (do WS).

To pozwoli szybko zweryfikować koncepcję "Distance is irrelevant" bez psucia obecnego pipeline'u produkcyjnego.
