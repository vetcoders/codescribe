# Codescribe Local Agent Contract

The Vetcoders Global Agent Charter is authoritative. This file adds only
Codescribe-specific runtime laws, thrones of authority, release cadence, and canonical pointers.

## Naming & Authority (Founder decision 2026-08-28)

- **Founder** = Maciej Gad and Monika Szymańska (human voice, decisions, buttons).
- **Operator** is exclusively an AGENT role (`vc-operator`, integrator). Never call the Founder "operator".
- **Prawo Cięcia**: Jeden tron na władzę, zero nowych warstw. Konkurent tronu jest bezwzględnie
  USUWANY (`git rm` / wycięcie symbolu), nigdy opakowywany.
- **Zakazane słowa w diffach, kodzie i commitach**: `shim`, `compat`, `legacy`, `adapter-for-old`,
  `fallback-to-previous`, `bridge-until`, `TODO remove`. Każde = odrzucony cut. Żadnych fikuśnych garbatych wrapperów.
- **Falsyfikator przed edycją**: test „pięć Iwo” (5 fizycznych wystąpień PCM → 5 w ledgerze → 5 w reducerze → 5 w delivery).
- Zobacz `CANARY_MAP.md` oraz `AGENT_CANARY.md` dla pełnej mapy kolizji i 7 tronów.

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
