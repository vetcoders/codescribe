# CodeScribe Resurrection Plan: 10 Faz Odbudowy Prawdy Produktu

Data: 2026-04-12

Cel nadrzędny:
- Aplikacja ma mówić prawdę swoją obietnicą na to, że użytkownik powie prawdę swoich intencji, a ona ich nie zgubi i nie zniekształci.

Artefakt źródłowy:
- Raport bazowy: [postmortem-transcriptions-2026-04-12.md](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:1)

Założenie robocze:
- Silnik umiał.
- Projekt zawodzi głównie tam, gdzie prawda silnika nie staje się prawdą produktu.

## Jak czytać ten plan

Każda faza ma:
- intencję
- konkretny rezultat
- pytanie o prawdę, które musi zostać domknięte
- odniesienia do sekcji raportu, z których wynika potrzeba tej fazy

## Audit stanu kodu — 2026-04-15

Audyt wykonany na branchu `feat/the-intents-engine` przeciwko aktualnemu kodowi, nie tylko commit message'om.

### Odhaczone w kodzie

- [x] Faza 1. Spisać Konstytucję Prawdy
- [x] Faza 2. Rozdzielić Draft od Werdyktu
- [x] Faza 3. Uczynić Provenance Częścią Artefaktu
- [x] Faza 4. Zmienić App z "Wybieracza Tekstu" w "Sędziego Prawdy"
- [x] Faza 5. Ujawnić Prawdę VAD i Braku Mowy
- [x] Faza 6. Ucywilizować Fallbacki
- [x] Faza 8. Rozdzielić Kategorię "Transcription" od Kategorii "Interpretation"
- [x] Faza 9. Zbudować Truth QA zamiast tylko STT QA
- [x] Faza 10. Przepisać Obietnicę Produktu na Język UI i Onboardingu

### Częściowo dowiezione

- Faza 7. Zatrzymać Korekty, Które Pogarszają
  Guardraile dla final pass i commit quality istnieją, ale nie wszystkie długie ścieżki transkrypcji są jeszcze verdict-first.

## Faza 1. Spisać Konstytucję Prawdy

Intencja:
- Zanim zmienimy routing, musimy nazwać, co w CodeScribe oznacza "nie zgubić" i "nie zniekształcić".

Rezultat:
- Krótki spec produktu: `truth-contract.md`
- Definicje:
  - `intent-preserving`
  - `transcript`
  - `formatted transcript`
  - `assistant interpretation`
  - `no speech`
  - `low confidence`
  - `fallback`

Pytanie o prawdę:
- Kiedy aplikacja ma prawo coś wkleić automatycznie, a kiedy ma obowiązek powiedzieć: "nie mam pewności"?

Odniesienia do raportu:
- [Current State / 1. The app does not have one transcript truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:34)
- [What The App Gives Today](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:202)
- [Proposal](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:249)

## Faza 2. Rozdzielić Draft od Werdyktu

Intencja:
- Live preview jest użyteczny, ale nie może po cichu stawać się ostateczną prawdą.

Rezultat:
- Jawny model dwóch stanów:
  - `draft` = streaming/live
  - `verdict` = wybrany finalny transcript
- Zmiana UI i logiki zapisu tak, by użytkownik widział różnicę.

Pytanie o prawdę:
- Czy to, co user widzi "na żywo", jest tylko podglądem, czy czymś, co app obiecuje jako finalny zapis intencji?

Odniesienia do raportu:
- [Current State / 1. The app does not have one transcript truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:34)
- [Current State / 3. Cloud failure currently degrades to streaming truth without making the downgrade legible](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:93)
- [Proposal](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:249)

## Faza 3. Uczynić Provenance Częścią Artefaktu

Intencja:
- Każdy zapisany transcript musi mówić skąd pochodzi.

Rezultat:
- Każdy transcript dostaje obok siebie albo w sidecar metadata:
  - `source`
  - `engine`
  - `mode`
  - `fallback_used`
  - `vad_speech_pct`
  - `no_speech_reason`
  - `confidence_flags`

Pytanie o prawdę:
- Jeśli za tydzień patrzymy na plik, czy umiemy odpowiedzieć, dlaczego właśnie taki tekst został uznany za finalny?

Odniesienia do raportu:
- [Current State / 2. The product already knows when audio is mostly silence, but hides that truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:58)
- [Current State / 3. Cloud failure currently degrades to streaming truth without making the downgrade legible](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:93)
- [Is The Bottleneck In `app` Or `core`?](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:258)

## Faza 4. Zmienić App z "Wybieracza Tekstu" w "Sędziego Prawdy"

Intencja:
- `app` ma przestać być miejscem cichego wyboru między różnymi tekstami i stać się jawnym adjudikatorem.

Rezultat:
- Jeden moduł decyzyjny odpowiedzialny za:
  - wybór finalnego transcriptu
  - oznaczanie degradacji
  - blokadę auto-paste przy braku pewności
- Koniec z rozproszoną logiką "tu fallback, tam save, gdzie indziej paste".

Pytanie o prawdę:
- Czy da się jednym miejscem w kodzie odpowiedzieć: "dlaczego wkleiłem dokładnie to"?

Odniesienia do raportu:
- [Current State / 1. The app does not have one transcript truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:34)
- [Is The Bottleneck In `app` Or `core`?](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:258)
- [Migration Plan](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:298)

## Faza 5. Ujawnić Prawdę VAD i Braku Mowy

Intencja:
- Jeżeli system wie, że prawie nic nie było mową, user musi to usłyszeć od produktu.

Rezultat:
- Nowe stany produktu:
  - `No speech`
  - `Very low speech`
  - `Possible hallucination`
- Auto-paste wyłączony dla tych stanów.
- Archiwum przechowuje verdict, nie tylko tekst.

Pytanie o prawdę:
- Czy w sytuacji `0-6% speech` app dalej ma prawo udawać, że "transkrypcja się udała"?

Odniesienia do raportu:
- [Current State / 2. The product already knows when audio is mostly silence, but hides that truth](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:58)
- [What The App Could Give](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:219)
- [Quick Win](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:320)

## Faza 6. Ucywilizować Fallbacki

Intencja:
- Fallback jest dozwolony. Ukryty fallback jest zdradą obietnicy produktu.

Rezultat:
- Każdy fallback dostaje klasę:
  - `acceptable fallback`
  - `degraded fallback`
  - `unsafe fallback`
- Tylko `acceptable fallback` może iść do auto-paste.
- `degraded` i `unsafe` trafiają do review state albo wymagają potwierdzenia.

Pytanie o prawdę:
- Czy użytkownik wie, że czyta efekt awarii ścieżki głównej, a nie normalny wynik?

Odniesienia do raportu:
- [Current State / 3. Cloud failure currently degrades to streaming truth without making the downgrade legible](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:93)
- [What The App Gives Today](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:202)
- [Migration Plan](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:298)

## Faza 7. Zatrzymać Korekty, Które Pogarszają

Intencja:
- "Postprocess" i "cleanup" nie mogą mieć immunitetu. Muszą udowodnić, że pomagają.

Rezultat:
- Guardrail porównujący:
  - raw
  - postprocessed
  - final
- Jeśli korekta:
  - wprowadza obce tokeny
  - obniża spójność języka
  - pogarsza strukturę sensu
  - albo zwiększa drift bez zysku,
  to zostaje odrzucona.

Pytanie o prawdę:
- Czy system umie powiedzieć: "moja korekta była gorsza, więc jej nie użyłem"?

Odniesienia do raportu:
- [Current State / 4. Post-processing sometimes makes a better transcript worse](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:124)
- [What The App Could Give](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:219)
- [Migration Plan](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:298)

## Faza 8. Rozdzielić Kategorię "Transcription" od Kategorii "Interpretation"

Intencja:
- Dziś archiwum miesza transcript, formatting i assistive output. To jest poznawczo nieuczciwe.

Rezultat:
- Oddzielne, nazwane powierzchnie:
  - `Transcript`
  - `Formatted transcript`
  - `Assistant interpretation`
  - `AI failed, raw preserved`
- Oddzielne ścieżki UI i eksportu.

Pytanie o prawdę:
- Czy user może po pliku i UI od razu poznać, czy czyta zapis tego, co powiedział, czy interpretację tego, co system zrozumiał?

Odniesienia do raportu:
- [Current State / 5. AI is powerful, but the product mixes categories](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:154)
- [What The App Gives Today](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:202)
- [What The App Could Give](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:219)

## Faza 9. Zbudować Truth QA zamiast tylko STT QA

Intencja:
- Projekt nie potrzebuje już tylko testów "czy coś wyszło". Potrzebuje testów "czy system nie skłamał produktem".

Rezultat:
- Nowy zestaw evali i fixture'ów:
  - duży WAV -> mały TXT
  - cloud failure -> streaming fallback
  - final-pass lepszy niż streaming
  - no-speech
  - low-confidence
  - raw lepszy niż postprocessed
- Każdy fixture ma oczekiwany werdykt produktowy, nie tylko oczekiwany tekst.

Pytanie o prawdę:
- Czy pipeline testuje dziś prawdomówność produktu, czy tylko techniczną przechodniość?

Odniesienia do raportu:
- [Corpus Snapshot](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:14)
- [Current State](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:32)
- [Is The Bottleneck In `app` Or `core`?](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:258)

## Faza 10. Przepisać Obietnicę Produktu na Język UI i Onboardingu

Intencja:
- Resurrection nie kończy się na kodzie. Produkt musi obiecywać dokładnie to, co potrafi dowieźć prawdziwie.

Rezultat:
- Nowy wording onboardingowy i statusowy:
  - "Live preview"
  - "Final local pass"
  - "Fallback result"
  - "No reliable speech detected"
  - "Assistant interpretation, not verbatim transcript"
- User ma rozumieć system nie przez dokumentację techniczną, tylko przez interfejs.

Pytanie o prawdę:
- Czy obietnica produktu jest zbieżna z realnym zachowaniem systemu w edge case'ach?

Odniesienia do raportu:
- [What The App Gives Today](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:202)
- [What The App Could Give](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:219)
- [Quick Win](/Users/polyversai/Libraxis/CodeScribe/docs/postmortem-transcriptions-2026-04-12.md:320)

## Sekwencja Wykonawcza

Jeśli mamy to robić bez rozmywania energii, sugerowana kolejność jest taka:

1. Faza 1
2. Faza 2
3. Faza 3
4. Faza 4
5. Faza 5
6. Faza 6
7. Faza 7
8. Faza 8
9. Faza 9
10. Faza 10

To nie jest przypadkowa lista.

Najpierw:
- definiujemy prawdę
- potem ujawniamy źródło i werdykt
- potem usuwamy miejsca, gdzie app kłamie fallbackiem lub korektą
- dopiero potem stroimy AI i komunikację

## Najostrzejszy Wniosek

Jeśli projekt ma zmartwychwstać, nie potrzebuje przede wszystkim "lepszego STT".
Potrzebuje produktu, który:

- nie myli draftu z werdyktem
- nie myli transcriptu z interpretacją
- nie myli fallbacku z sukcesem
- nie myli krótkiego tekstu z pewną prawdą

Resurrection CodeScribe to nie będzie "więcej modeli".
To będzie odzyskanie moralnej spójności między:

- intencją użytkownika
- werdyktem silnika
- decyzją aplikacji
- obietnicą interfejsu
