# CodeScribe Truth Contract

Data: 2026-04-21

Cel:
- CodeScribe ma zachowywać intencję użytkownika bez cichego mieszania podglądu, werdyktu i interpretacji.

## Słownik produktu

- `Live preview`
  - Lokalny, prowizoryczny podgląd pojawiający się w trakcie nagrania.
  - Nie jest automatycznie równy finalnej prawdzie.

- `Committed verdict`
  - Ostateczny transcript wybrany po zakończeniu nagrania.
  - To ten artefakt decyduje o zapisie, auto-paste i sidecarze prawdy.

- `Transcript`
  - Najwierniejszy możliwy zapis tego, co zostało wypowiedziane.

- `Formatted transcript`
  - Transcript po bezpiecznej obróbce formatowania.
  - Jeżeli korekta pogarsza tekst albo nic realnie nie wnosi, raw transcript wygrywa.

- `Assistant interpretation`
  - Odpowiedź asystenta oparta o transcript lub selekcję.
  - To nie jest transcript i nie może być etykietowana jak transcript.

- `No speech`
  - System nie ma wystarczających podstaw, by twierdzić, że w nagraniu była mowa.

- `Low confidence`
  - Transcript istnieje, ale system ma twarde sygnały, że jakość jest słaba.

- `Fallback`
  - Alternatywna ścieżka użyta dlatego, że ścieżka główna zawiodła albo nie była wystarczająca.

## Zasady

1. Live preview pozostaje lokalny i prowizoryczny.
2. Finalny zapis musi pochodzić z jawnie wybranego werdyktu po zakończeniu capture.
3. Degraded albo unsafe fallback nie może udawać normalnego sukcesu.
4. Silent auto-paste jest blokowany, gdy prawda jest słaba, niepełna albo niezweryfikowana.
5. AI formatting może ulepszać transcript, ale nie ma prawa po cichu nadpisywać raw truth.
6. Assistant output i transcript muszą pozostać osobnymi kategoriami w UI, archiwum i eksporcie.
7. Każdy zapisany transcript powinien zachować provenance, confidence flags i kontekst final-pass.

## Minimalne pytania kontrolne

- Czy user widzi różnicę między preview a committed verdict?
- Czy wiadomo, skąd pochodzi finalny tekst?
- Czy fallback jest nazwany jako fallback?
- Czy system umie powiedzieć "nie mam pewności" zamiast wklejać fikcję?
- Czy user rozpoznaje, czy czyta transcript, formatted transcript, czy interpretację?

## Artefakty prawdy

CodeScribe materializuje truth contract przez:

- runtime verdicts w `core/pipeline/contracts.rs`
- controller truth surfaces w `app/controller/*`
- sidecary `truth.json`
- wording w Settings, overlay, onboardingu i dokumentacji
