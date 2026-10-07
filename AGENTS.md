# Codescribe Local Agent Contract

The Vetcoders Global Agent Charter is authoritative. This file adds only
Codescribe-specific runtime laws, thrones of authority, release cadence, and canonical pointers.

## Naming & Authority

- **Founder** = Maciej Gad and Monika Szymańska (human voice, decisions, buttons).
- **Operator** is exclusively an AGENT role (`vc-operator`, integrator). Never call the Founder "operator".
- **Prawo Cięcia**: Jeden tron na władzę, zero nowych warstw. Konkurent tronu jest bezwzględnie
  USUWANY (`git rm` / wycięcie symbolu), nigdy opakowywany.
- **Unikamy w pracy: w diffach, kodzie i commitach**: `shim`, `compat`, `legacy`, `adapter-for-old`,
  `fallback-to-previous`, `bridge-until`, `TODO remove`. Kod pisze się szybko. Refaktor monstrualnych i pogmatwanych konstrukcji to męka.
- **Falsyfikator przed edycją**: test „pięć Iwo” (5 fizycznych wystąpień PCM → 5 w ledgerze → 5 w reducerze → 5 w delivery).
- Zobacz `CANARY_MAP.md` oraz `AGENT_CANARY.md` dla pełnej mapy kolizji i 7 tronów.

## Worker embargo — only for `stt engine` related tasks:

- **Worker pisze kod. Testy pisze i uruchamia integrator. Worker nie kompiluje
  i nie uruchamia żadnych testów.**
  Build, typecheck, Clippy, wykonanie produktu, modele, benchmarki, instalacja
  i odbiór runtime należą wyłącznie do jawnie wyznaczonego integratora.
- Zakaz obowiązuje przez cały przydział workera, także po zamknięciu W2.
  Brak markera embargo, mały zakres, neutralny instrument, szybki smoke test,
  błąd kompilatora albo zalecenie skilla nie tworzą wyjątku.
- Worker może czytać i mapować źródła, robić statyczny przegląd oraz
  `git diff --check`. Oddaje commit, zakres zmian i niepewności. Integrator
  odpowiada za fixtury, testy i ich wyniki; worker nie ogłasza ich jako PASS.
- Integrator uruchamia wymagane bramki na jawnie wskazanej generacji po odbiorze
  zmian. Po błędzie może zwrócić workerowi cut do poprawy; worker nadal nie
  uruchamia bramek. Worker nie mianuje sam siebie integratorem.
- Każdy dispatch musi zawierać tę zasadę, rolę oraz tożsamość integratora.
  Dotyczy wszystkich providerów i runtime'ów. Hook uruchamiający kompilację
  lub testy również podlega zakazowi; checkpoint stosuje protokół z
  `docs/COMPILE_EMBARGO.md`, z jawną listą pominiętych hooków.
- Wymagania build/test/install w tym pliku wykonuje **integrator**, nie worker.
  Szczegóły i granica technicznego egzekwowania: `docs/COMPILE_EMBARGO.md` §0.

## Trony władzy (Runtime authority)

- `acoustic_ledger.rs` (`core/pipeline/acoustic_ledger.rs`): jedyny tron tożsamości PCM (`OccurrenceIdentity`, `ObservationIdentity`, `MutationReceipt`). Tekst jest etykietą przypiętą do occurrence, nigdy kluczem identity.
- **Seal authority**: predykat na energii PCM + dolinach Silero VAD na tym samym capture epoch, nigdy na równości stringów. Równość stringów nie tworzy, nie łączy i nie kasuje occurrence.
- `RecordingController`: jedyny właściciel mikrofonu w aplikacji. Dictation, Agent i Assistive mogą routować downstream; żaden nie tworzy własnego recordera.
- `PresentationEmitter` (`TranscriptReducer`): jedyny reducer dokumentu. `OverlayState.swift` to wyłącznie projekcja i malowanie UI bez własnego stanu dokumentu.
- `TranscriptBus`: wyłącznie obserwator zatwierdzonych zdarzeń reducera. Zero reinterpretacji dokumentu.
- `delivery_route.rs` (`app/controller/delivery_route.rs`): jedyny tron routingu delivery. Delivery kieruje się jawną intencją Foundera / użytkownika, nigdy samym fokusem OS.
- `settings.json` → loader → immutable runtime snapshot: jedyne źródło konfiguracji runtime. Zero pięciogłosu.
- Terminal events zwalniają stan mikrofonu, fazę UI i wątek Agenta.

## Canonical contracts

- `CANARY_MAP.md` — mapa kolizji, 20 konkurentów i 7 tronów.
- `docs/STT_CONTRACT.md` — silniki i adjudykacja.
- `docs/TRANSCRIPT_BUS.md` — czyste zdarzenia, prywatność i ścieżki.
- `docs/HOTKEYS_CONTRACT.md` — gesty, ownership i tryby.
- `docs/DELIVERY_ROUTE.md` — destination selection.
- `docs/ENV_REGISTRY.toml` — rejestr zmiennych środowiskowych.
- `docs/LOCALIZATION.md` — język interfejsu: String Catalogs, reguły pisania copy, ledger.

Po zmianach w API Rust bridge regeneruj bindingi Swift przez `make app-bindings`.

## Daily app and release cadence

- Po spójnym cucie zmieniającym aplikację uruchom `make install-if-idle`. Odmawia
  tylko podczas trwającego nagrywania (Transcript Bus) lub aktywnej tury agenta
  (`~/.codescribe/agent-turn.lock`); samo działanie aplikacji nie blokuje (Founder, 2026-09-08).
- Traktuj instalację jako wymagany odbiór dla Foundera przy każdym większym cucie.
  Zweryfikuj wersję, build, commit, podpis i pomyślny start `/Applications/Codescribe.app`;
  dopiero wtedy odtwórz `/usr/bin/afplay /System/Library/Sounds/Ping.aiff`.
- Odmów instalacji, gdy trwa nagranie: aktywna sesja nie ma `session_ended`
  (historyczny unpaired `transcript_sealed` nadal się liczy), trwa sesja `cli_file_verdict`
  lub aplikacja trzyma blokadę runtime. Nigdy nie ubijaj aplikacji w trakcie take'a.
- Co najwyżej jeden `make release-standard` (notaryzowany slim DMG) dziennie,
  gdy bus jest idle. Wypuszczaj tylko na wyraźne polecenie Foundera.
- Ad-hoc build `/Applications` to nie jest dystrybucyjny DMG. Produkcyjny DMG
  wymaga podpisu, notaryzacji, sumy kontrolnej, staplingu i `verify-dmg`.

## Verification

- `make check` — formatowanie, Clippy, Semgrep, rejestr env i gate ledger.
- `make verify` — hermetyczne testy Rusta i doctesty (kontrakt CI).
- `make test-swift` — regeneracja bindingów oraz pakiet testów Swifta (Swift 6 strict concurrency, warnings as errors).
- Swift targets budują się bez wyciszania ostrzeżeń; warning suppressors są zabronione.
