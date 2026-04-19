# CodeScribe: Truth Convergence Plan (Marbles)

## Nadrzędny Cel
Aplikacja ma mówić prawdę swojej obietnicą na to, że użytkownik powie prawdę swoich intencji. Likwidujemy luki (Kłamstwa) między tym, co wie silnik STT i VAD, a tym, co system faktycznie zwraca i loguje.

## Lista Kłamstw (Lies) do usunięcia

**Kłamstwo 1: Silnik lokalny nadal ukrywa metadane (Spłaszczanie do `Stringa`)**
- Gdzie: `core/stt/whisper/singleton.rs`, `core/stt/mod.rs` (`transcribe`, `transcribe_long`).
- Problem: Zwracają `Result<String>` zamiast `Result<RawTranscript>`, gubiąc `logprob` i `compression_ratio`.
- Naprawa: Zmienić sygnatury na `Result<RawTranscript>` i zaktualizować miejsca wywołania (np. serwer IPC).

**Kłamstwo 2: Niestrzeżone Formatowanie AI (Brak *Correction Guard*)**
- Gdzie: `app/controller/mod.rs` (`ai_formatting::format_text_with_status`).
- Problem: System ślepo przypisuje wynik z LLM jako nową prawdę, nie weryfikując, czy AI nie wycięło połowy zdania.
- Naprawa: Wprowadzić walidację różnicy długości/jakości między `raw_text` a wyjściem AI, odrzucając formatowanie, jeśli jest drastycznie różne.

**Kłamstwo 3: Prawda Chmury jest bytem drugiej kategorii**
- Gdzie: `core/llm/client.rs` (`transcribe_cloud`).
- Problem: Zwraca `Result<String>`.
- Naprawa: Powinno zwracać ustrukturyzowany werdykt, oznaczony twardo jako `TranscriptionSource::CloudPrimary`.

**Kłamstwo 4: Narzędzia analityczne okłamują same siebie (`qube_report.rs`)**
- Gdzie: `bin/qube_report.rs`.
- Problem: Jeśli STT odrzuci tekst przez niski `logprob` (Quality Gate), ewaluator traktuje to jako 100% błąd rozpoznawania mowy (pusty tekst).
- Naprawa: Ewaluator musi rozróżniać pusty tekst z powodu braku mowy/halucynacji od błędu STT, konsumując `RawTranscript`.

**Kłamstwo 5: Strumieniowanie jako substytut prawdy (The Streaming Illusion)**
- Gdzie: `app/controller/mod.rs` (Fallback logic).
- Problem: Fallback strumieniowy nie ma wyraźnego odcięcia strukturalnego. Strumień jest zbiorem nakładających się tokenów, bez własnego `logprob`.
- Naprawa: Wyraźniej oflagować tekst pochodzący ze strumienia (w `confidence_flags`) jako `UnverifiedStream`, aby UI nie traktowało go na równi z final-pass.

**Kłamstwo 6: Brak Immunitetu dla krótkich wypowiedzi (The "Format Everything" Lie)**
- Gdzie: `app/controller/mod.rs` (obsługa `force_ai` / lewy Option).
- Problem: Krótkie zwroty ("ok", "idziemy") są wysyłane do LLM-a, który dopowiada kontekst.
- Naprawa: Wprowadzić Fast Reject ("Immunitet") dla tekstów poniżej N znaków, chroniąc je przed zniekształceniem.

**Kłamstwo 7: Utrata metadanych VAD (Sparkline drop)**
- Gdzie: `app/controller/types.rs` -> `RecordingTruthVerdict`.
- Problem: `adjudicate_recording_truth` przyjmuje `VadVerdict` ze sparklinem, ale gubi go podczas mapowania do ostatecznego werdyktu dla UI i historii.
- Naprawa: Przepiąć `sparkline` do `RecordingTruthVerdict` i zapisać w `truth.json`.

**Kłamstwo 8: Zrównanie błędu sieci z brakiem mowy w chmurze**
- Gdzie: `app/controller/mod.rs` (Awaiting cloud STT).
- Problem: Gdy chmura zwraca pusty ciąg (brak mowy), jest to traktowane tak samo jak `Err(Network)`, powodując ślepy fallback do strumienia.
- Naprawa: Rozróżnić pusty werdykt chmurowy (prawdziwy brak mowy) od technicznego błędu sieci, aby nie wklejać halucynacji ze strumienia.

**Kłamstwo 9: Niszczenie bezpieczeństwa typów w logach prawdy**
- Gdzie: `app/controller/types.rs`.
- Problem: `TranscriptionConfidenceFlag` (enum) jest konwertowany na gołe, łatwe do pomylenia stringi (`Vec<String>`) w `RecordingTruthVerdict`.
- Naprawa: Przenieść typ `TranscriptionConfidenceFlag` do API wyższego poziomu (lub zduplikować go z serde) i używać silnych typów na całej ścieżce aż do `truth.json`.

## Zasady Wykonania (vc-marbles)
- Pętla izolowana (loop until done).
- Napraw jedno kłamstwo na raz.
- Uruchom jakość (`cargo check --workspace` / `cargo test`).
- Zacommituj.
- Przejdź do następnego kłamstwa.
