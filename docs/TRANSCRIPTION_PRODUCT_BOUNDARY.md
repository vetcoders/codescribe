# Transkrypcja działa znakomicie. Stawiamy na nogi wszystko wokół niej

> Status: decyzja Foundera z 2026-08-29
> Zakres: granica produktu, lifecycle, Bus, recovery, GUI i komunikacja z agentem

## Decyzja

Jakość transkrypcji jest obecnie najlepsza w historii Codescribe. Nie stroimy
Apple, Whispera, Lexiconu ani warstw formatowania tekstu w ramach prac nad
awariami obudowy produktu. Nie zamieniamy dobrego tekstu w pole eksperymentu,
gdy zawodzą lifecycle, zapis pliku, terminalizacja, Bus albo GUI.

Naprawiamy wszystko wokół transkrypcji:

- trwały zapis całego nagrania;
- jednoznaczne zakończenie sesji;
- Transcript Bus i provenance każdej koperty;
- recovery oraz Retranscribe;
- uczciwe statusy i błędy w GUI;
- komunikację agenta opartą na zdarzeniach.

## Jedna ścieżka produktu

```text
RecordingController zatrzymuje mikrofon
  -> pełny WAV otrzymuje trwały receipt
  -> PresentationEmitter domyka wynik albo opisuje recovery
  -> Transcript Bus publikuje terminalne zdarzenie z prawdziwym source
  -> event-triggered monitor budzi nazwany thread agenta
  -> agent otrzymuje pełną kopertę
  -> odpowiedź tekstowa lub głosowa wraca do Foundera
```

Awaria późniejszej warstwy nie może cofnąć prawdy warstwy wcześniejszej.
Jeśli recorder zapisał poprawny WAV, plik nie może pozostać anonimowym artefaktem
w `/private/var/folders` tylko dlatego, że terminalny transcript seal był
niepełny.

## Zasada monitora: Event triggered

Monitor agenta jest wybudzany przez nowe zdarzenie. Cykliczne odpytywanie pliku
nie jest monitorem docelowym i nie uprawnia do deklaracji "nasłuchuję".

Wymagany kontrakt:

1. Publikacja zdarzenia uruchamia dostarczenie bez oczekiwania na kolejny turn
   użytkownika, ręczny command albo poll narzędzia.
2. Cursor i replay służą wyłącznie do recovery po przerwie; nie są normalnym
   mechanizmem wybudzania.
3. Do threadu agenta trafia tylko kompletna ramka
   `CODESCRIBE_BUS_ENVELOPE_BEGIN/END`.
4. Koperta zachowuje co najmniej `source`, `kind`, `audience`, `session_id`,
   `delivery_id`, `state_change_allowed` i `text`.
5. Zmiany stanu są dozwolone tylko dla `kind=seal` oraz
   `state_change_allowed=true`.
6. Nazwa agenta może wystąpić w dowolnym miejscu wypowiedzi i pozostaje filtrem
   skrzynki, a nie mechanizmem transportu.
7. `source=cli_file_verdict` nie może udawać live take'a aplikacji. Producent
   zdarzenia jest zachowywany end-to-end.

Obecny `bus-demux.py --follow` utrzymuje cursor i filtruje koperty poprawnie,
ale jego interval-based follow nie spełnia zasady Event triggered. Jest
mechanizmem przejściowym, nie dowodem zakończonego monitora.

## Incydent referencyjny

W sesji `a7b48906-38af-44e7-b669-1aab5a5240f5` recorder:

- zatrzymał stream poprawnie;
- zapisał 35.818667 s mono PCM 48 kHz Int16;
- pozostawił pełny WAV w katalogu tymczasowym;
- nie zaktualizował `~/.codescribe/last_session.wav`.

Późniejszy terminalny seal zaraportował
`terminal_seal_coverage_incomplete` (`max_uncovered=50688`, próg `12000`). Bus
zakończył sesję jako `sealed=false`, a GUI pokazało mylące
`Failed to stop recorder`. To nie była awaria zatrzymania mikrofonu. To była
awaria terminalizacji po poprawnym zapisie audio.

Ten incydent wyznacza test regresyjny całej obudowy: dobry WAV musi zostać
nazwany, zachowany i zaoferowany do Retranscribe nawet wtedy, gdy transcript
seal nie może zostać uznany za kompletny.

## Stan pierwszego cięcia

Commit `5040e4ba` zachowuje pole `source` z kanonicznego Transcript Busa w
kopercie demuxa. Dzięki temu `cli_file_verdict`, live app take i przyszłe
recovery nie są zlewane w jedną fałszywą tożsamość producenta.

To jest pierwszy naprawiony fragment obudowy. Nie zmienia tekstu transkrypcji
ani żadnego silnika STT.

## Kryteria zakończenia

- pełny WAV jest promowany do recovery przed ryzykowną terminalizacją tekstu;
- `last_session.wav` albo jawny recovery artifact wskazuje najnowszy take;
- niepełny seal nie jest raportowany jako awaria recordera;
- Bus zawsze domyka lifecycle i zachowuje prawdziwy producer source;
- Retranscribe potrafi użyć recovery artifact bez ręcznego szukania w `/tmp`;
- event-triggered monitor sam budzi właściwy thread po adresowanym sealu;
- test end-to-end nie wymaga ręcznego `codescribe transcribe <file>`;
- jakość i tekst transkrypcji pozostają bez zmian.
