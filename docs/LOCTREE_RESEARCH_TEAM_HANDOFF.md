---
title: Loctree research team handoff
date: 2026-08-24
language: pl
project: vetcoders/codescribe
baseline: dbxms-runtime-claude@94eb5af0d99daf5642aab8bf2cc320b2b14ea16d
research_gate: PASS_FOR_RED_ONLY
status: metodologia-zamrozona-preflight-przed-implementacja
---

# LOCTREE RESEARCH TEAM HANDOFF

## 1. Po co istnieje ten dokument

To jest kanoniczny, współdzielony handoff dla zespołu badającego **agentic
silent functional smear** i dla zespołu naprawiającego Codescribe. Zastępuje
konieczność odtwarzania 28 tysięcy linii rozmowy, ale nie zastępuje źródeł.

Dokument rozdziela dwa strumienie pracy:

1. **program badawczy Loctree/Canary** — cztery z góry wybrane repozytoria,
   wspólna metodologia, kontrole dodatnie i ujemne;
2. **naprawę Codescribe** — zamknięty preflight, syntetyczny RED, niezależna
   replikacja na Dragonie, a dopiero potem implementacja jednego tronu.

Nie otwieramy kolejnego researchu dla Codescribe. R0 jest zamknięte jako
`PASS_FOR_RED_ONLY`. Dragon nie jest piątą ścieżką opinii. Dragon ma niezależnie
odtworzyć ten sam RED na tym samym commicie.

## 2. Receipt i stan bramki

Stan uchwycony 2026-08-24 09:00 CEST:

| Pole          | Wartość                                               |
| ------------- | ----------------------------------------------------- |
| Repo          | `/Users/maciejgad/vc-workspace/vetcoders/codescribe`  |
| Branch        | `dbxms-runtime-claude`                                |
| HEAD          | `94eb5af0d99daf5642aab8bf2cc320b2b14ea16d`            |
| Loctree       | `0.14.4+g3e9eb0a7`                                    |
| Snapshot      | świeży wobec HEAD, lecz worktree dirty                |
| Dirty receipt | `dirty:4:sha256:0fbf15a1d45c30a3`                     |
| R0            | `[x] PASS_FOR_RED_ONLY`                               |
| P0-A          | `[ ]` — substrate nieuporządkowany                    |
| P0-B          | `[ ]` — publiczny `five-Iwo` RED jeszcze nie istnieje |
| Dragon RED    | `[ ]` — nie wolno uruchamiać przed commitem P0-B      |
| W1-A/B/C      | `[ ]` — implementacja jeszcze niedopuszczona          |

Dirty surfaces są obecnie cztery, nie trzy:

- `.gitignore` — dwie prywatne fixture paths;
- `AGENTS.md` — rozrośnięty lokalny kontrakt;
- `docs/ENV_REGISTRY.toml` — zmiana domyślnego embeddings switcha;
- `.loctignore` — nieśledzony eksperyment `!/*.md`, który nie odwraca reguły
  `/*.md` z `.gitignore`.

P0-A musi rozstrzygnąć każdą z nich jawnie. Nie wolno po cichu usuwać
`.loctignore`, stashować cudzych zmian ani nazywać drzewa czystym, zanim takie
nie będzie.

## 3. Źródło rozmowy i jak je odtworzyć

Pełny ekstrakt bieżącej sesji został wygenerowany dokładnie tak:

```bash
aicx extract codex --session "$(aicx sessions current)" --conversation
```

Wynik:

```text
/Users/maciejgad/.aicx/extracts/codex/01a01fbf-0d22-7f22-96b3-dbc359eb3595_conversation.md
28 369 linii · 1 409 853 bajtów
sha256 6a8851fe4d18c8a5af5ffd93788d4022f14b653c8e6725bcda0c1ba775733a5d
```

AICX jest tu historią intencji i pochodzenia decyzji, nie prawdą bieżącego
kodu. Każde twierdzenie z rozmowy wymaga świeżego receipt Git/Loctree/runtime.

## 4. Ustalony język pojęć

### 4.1. Tron

Tron to jedyny komponent mający prawo rozstrzygać określony rodzaj prawdy:
tożsamość, istnienie, finalność, redukcję dokumentu, ustawienia albo delivery.
Czytelnik, cache, transport i projekcja nie są tronami, dopóki nie podejmują
własnej decyzji lub mutacji.

### 4.2. Smear

**Agentic silent functional smear** to użyteczna synteza istniejących klas
problemów: semantic clones, feature interactions, semantic merge conflicts i
rozproszonego autorytetu w pracy równoległych agentów. Nie twierdzimy, że jest
to nowa klasa matematyczna ani termin już uznany naukowo.

Smear występuje, gdy lokalnie sensowne i często CI-green komponenty niezależnie
rozstrzygają tę samą prawdę, a wynik zintegrowany zależy od kolejności, ścieżki,
cache, transportu lub ostatniego mutatora. Zielone CI nie jest falsyfikatorem
smearu, jeżeli testy nie obejmują prawdy produktowej end-to-end.

### 4.3. Projection, authority i transport

Każdy kandydat musi dostać dokładnie jedną rolę w badanym domainie:

- `writer` — tworzy lub zmienia kanoniczny byt;
- `arbiter` — wybiera, która obserwacja jest ważna lub finalna;
- `projector` — odwzorowuje już rozstrzygniętą prawdę;
- `cache` — przechowuje pochodną z jawną rewizją;
- `transport` — przenosi dane bez reinterpretacji;
- `generated/schema mirror` — legalna kopia formatu;
- `test/diagnostic` — osądza lub raportuje, ale nie mutuje runtime.

Podobne nazwy nie tworzą smearu. Różne nazwy nie wykluczają smearu.

## 5. Teoria akustyczna — właściwy, nierozcięty operator

Wcześniejsza synteza popełniła błąd metodologiczny: rozdzieliła energię od
Silero, a potem atakowała sam RMS szumem, kaszlem lub muzyką. To falsyfikowało
okrojoną tezę, której operator nie postawił.

Właściwy przedmiot badania jest wspólny:

```text
kanoniczny capture PCM x[n]
  + Silero/VAD(model, wersja, próg, histereza, min-speech, min-silence)
  + całka energii po dokładnie tym samym, kompletnie pokrytym zakresie
  + kanoniczny zegar próbek i capture epoch
```

Niech Silero wyznaczy region mowy `R = [s, e)` w jednej epoce przechwycenia, a:

```text
E(R) = suma po n należących do [s,e) z |x[n]|²
```

Wtedy:

- istnienie occurrence wymaga dodatniego wyniku bramki mowy Silero oraz
  energii powyżej skalibrowanego progu na tym samym zakresie;
- tożsamość occurrence to wyłącznie
  `(session_id, capture_epoch, [sample_start, sample_end))`;
- wersja/model/progi Silero i receipt energii są dowodem admission, a nie częścią
  equality/hash;
- tekst jest mutowalną etykietą obserwatora przypiętą do occurrence;
- równość stringów nigdy nie tworzy, nie łączy i nie usuwa occurrence;
- VAD valley po `e` jest dowodem zamknięcia frontu, nie decyzją leksykalną;
- seal wymaga także dowodu, że dla zakresu nie może już legalnie przyjść
  obserwacja w otwartym frontierze.

Szum, kaszel i muzyka nie są logicznym kontrargumentem wobec całego operatora.
Są kontrolami kalibracyjnymi Silero. Jeżeli poprawnie zdefiniowana kontrola
non-speech przejdzie bramkę mowy, pada konfiguracja/model VAD dla danego
środowiska — nie tożsamość zakresu.

Granica uczciwości pozostaje ważna: region VAD nie musi być jednym słowem.
Publiczny `five-Iwo` RED ma dlatego używać pięciu rozłącznych, kontrolowanych
regionów mowy z pięcioma dolinami. Szybkie słowa bez doliny wymagają później
jawnych word pins/alignment; nie wolno wymyślać pięciu bytów z oczekiwanego
stringa.

## 6. Prawo zachowania Codescribe

Dla syntetycznego korpusu pięciu rozłącznych regionów, z których każdy niesie
etykietę `Iwo`:

```text
PCM regions       5
ledger occurrences 5
reducer entries    5
Bus commits        5
overlay labels     5
delivery labels    5
```

Ani cztery, ani sześć. Apple i Whisper mogą różnie nazwać ten sam zakres, ale
nie mogą zmienić liczby occurrence. Replay tej samej observation identity nie
może zwiększyć liczności. Korekta przed seal zmienia etykietę, nie byt. Mutacja
automatyczna po seal jest odrzucana. Ręczna korekta człowieka ma osobne jawne
provenance.

## 7. Co już wiemy o awarii Codescribe

### Obserwowane

- W module `acoustic_ledger.rs` istnieją lokalne testy i typy, ale
  `AcousticLedger`, `ObservationIdentity`, `MutationReceipt` i
  `ConservationTally` nie mają produkcyjnych konsumentów poza tym plikiem.
- Produkcyjny capture nadal przekazuje `capture_epoch = 0` w dwóch miejscach i
  potrafi odtworzyć `sample_start` z sekund dekodera zamiast odziedziczyć
  współrzędne kanonicznego capture clock.
- `SessionEnergyClock` i `SileroIngress` są oddzielnymi substrate'ami; energia
  nie ma session/epoch, a ledger nie posiada wspólnego predykatu Silero × area.
- Transcript Bus wykonuje własną decyzję coverage i potrafi wyczyścić wszystkie
  word spans, gdy jednemu brakuje energii.
- Rustowy `PresentationEmitter::TranscriptReducer` i Swiftowy `OverlayState`
  są dwoma reducerami dokumentu; Bus i delivery obserwują późniejsze projekcje.
- Długie nagranie zachowało odzyskiwalne PCM. Późniejszy file pass odzyskał
  wielokrotnie więcej treści niż live delivery. To umieszcza dominującą porażkę
  po capture: w admission, seal, reducerze lub ich integracji.

### Wnioskowane, nieudowodnione

- Mamy mocny pozytywny control rozproszenia autorytetu i finalności.
- Nie mamy jeszcze dowodu, że przyczyną jest współbieżny memory race; równie
  dobrze może to być deterministyczna konkurencja semantyk i kolejności.
- Nie mamy jeszcze produktu Loctree, który przewiduje awarię. Mamy narzędzia,
  które pomagają wystawić kandydatów do falsyfikacji.

## 8. Cztery z góry ustalone role repozytoriów

Repozytoria nie zostały dobrane po zobaczeniu wyniku.

| Repo                 | Rola w eksperymencie                         | Najważniejszy kandydat                                             |
| -------------------- | -------------------------------------------- | ------------------------------------------------------------------ |
| Codescribe           | dodatnia kontrola runtime i cel naprawy      | identity/seal/reducer/delivery                                     |
| AICX                 | kontrola Rust-heavy z czytelnym import graph | utrata role provenance przy mapowaniu authority                    |
| Vibecrafted          | polyglot/disjoint false-negative challenge   | Python control-plane kontra Rust `compute_view` i finality readers |
| Vista + Vista Portal | największy przewidywany smear domenowy       | drugi writer wizyty i cross-repo licensing authority               |

### 8.1. AICX

AICX nie jest prawdą stanu kodu. Jest wersjonowaną prawdą pochodzenia intencji
operatora dotyczącej kodu. Loctree mówi, co istnieje; AICX przechowuje, co miało
istnieć i dlaczego. Overlay powinien ujawniać zgodność albo fracture.

Najmocniejszy kandydat: `overlay.rs` pobiera `UserMsg` i `AgentReply`, lecz
`IntentRecord` nie zachowuje roli autora. Następnie authority jest wyprowadzane
z `IntentKind`, przez co agentowe `Decision` może otrzymać
`operator_confirmed`. `loct twins` tego nie wykrył. Jest to test obowiązkowy dla
detektora role-aware.

### 8.2. Vibecrafted

To najtrudniejszy przypadek dla samego grafu importów. Semantyczne krawędzie
biegną przez JSON/JSONL, filesystem, env, CLI/subprocess, HTTP/SSE/MCP, FFI i
zainstalowane kopie.

Najmocniejszy kandydat: kontrakt wskazuje Python `control_plane.py` jako ownera
run lifecycle, podczas gdy Rust `control-core/src/read.rs::compute_view()` sam
rekonstruuje bieżący stan z meta/lock/state/events/PID. TUI i MCP posiadają
dalsze predykaty finalności. Test musi odróżnić ten przypadek od legalnych
schema mirrors, generated FFI i jawnych compatibility copies.

### 8.3. Vista

Vista jest jednym produktem rozłożonym na dwa repozytoria i kilka runtime'ów.
Loctree uruchomione w root workspace nie zobaczy najważniejszych cross-repo
krawędzi, więc każdy trial wymaga osobnych receiptów dla `vista` i
`vista-portal` oraz ręcznego edge ledgeru.

Najmocniejszy dodatni kandydat: kanoniczna komenda tworzenia/edycji wizyty ma
permission, transakcję, CAS, walidację i kanoniczny event, natomiast agentowe
`visit_tools.rs` wykonuje własny `INSERT`/`UPDATE` z innym zestawem invariants.
Drugim obszarem jest portalowy entitlement resolver kontra desktopowe
walidatory/cache.

## 9. Wave area × time — pomiar, nie metafora

Dla jednego badanego bytu przez cały lifecycle mierzymy:

- `P(t)` — liczbę żywych projekcji tej samej prawdy;
- `A(t)` — liczbę niezależnych komponentów zdolnych ją rozstrzygać lub mutować;
- `D(t)` — liczbę projekcji pozostających rozbieżnie po zmianie canonical truth.

Duże `P(t)` może być legalne. Smear zaczyna się, gdy `A(t) > 1` bez jawnego
arbitration contract albo `D(t) > 0`.

Do porównań zapisujemy pola powierzchni:

```text
authority_area = całka po czasie z max(A(t) - 1, 0)
divergence_area = całka po czasie z D(t)
projection_area = całka po czasie z P(t)  # kontekst, nie samodzielny verdict
```

W logach dyskretnych jest to suma wartości razy czas do następnego zdarzenia.
Nie zastępujemy tego liczbą plików w spoczynku.

## 10. Wspólna metodologia trialu

### Faza A — prerejestracja

1. Nazwij badany byt i jego kanoniczną identity.
2. Nazwij jeden oczekiwany tron oraz legalne projekcje.
3. Zapisz scenariusz dodatni, ujemny, niejednoznaczny i adversarial.
4. Zapisz oczekiwany wynik przed uruchomieniem.
5. Zamroź repo/HEAD/dirty receipt, Loctree build i snapshot fingerprint.
6. Zapisz wersję faktycznie wykonywanej binarki AICX; checkout version nie jest
   runtime receipt.

### Faza B — mapa

1. `loct context --full --markdown` i `loct repo-view`.
2. `loct focus` na domainie, następnie `slice`, `impact`, literal occurrences i
   bodies dla kandydatów.
3. `twins`, `crowd`, `prism` tylko jako generatory kandydatów.
4. Osobny rejestr kanałów poza AST: persistence, schema, env, CLI, HTTP, SSE,
   MCP, FFI, file copies i generated code.
5. Każdy plik klasyfikujemy jako writer/arbiter/projector/cache/transport/
   generated/test. Nie liczymy wszystkich jako authority.

### Faza C — runtime trace

1. Przeprowadź jeden byt od inputu do końcowego consumer payload.
2. Zachowaj identity, revisions, receipts, write sites, events i timestamps.
3. Wprowadź prerejestrowaną perturbację.
4. Zmierz `P(t)`, `A(t)`, `D(t)` i pola powierzchni.
5. Zapisz osobno true positives, true negatives, false positives i false
   negatives. Jeden similarity score jest niewystarczający.

### Faza D — interwencja

1. Usuń jednego konkurenta tronu; nie dodawaj synchronizatora.
2. Powtórz dokładnie ten sam trial i instrumenty.
3. Oczekuj spadku `authority_area`/`divergence_area` i poprawy product invariant.
4. Non-authority refactor o podobnym LOC jest kontrolą ujemną i nie powinien
   istotnie poruszyć miar authority.

### Faza E — verdict

Każdy przypadek kończy się jako:

- `confirmed_smear`;
- `legal_projection_or_transport`;
- `unresolved`;
- `detector_false_positive`;
- `detector_false_negative`.

## 11. Ograniczenia obecnych instrumentów

- Prism nie jest jeszcze instrumentem dyskryminującym. Kontrole niezwiązane z
  authority trafiały do tego samego wysokiego pasma co Codescribe.
- `loct twins` zwraca głównie imienniki i nie znalazł semantycznych identity
  twins Codescribe ani role-authority loss w AICX.
- `loct follow events` nie śledzi znanej ścieżki Rust fanout → reducer → IPC →
  Swift.
- Graf importów dramatycznie zaniża cross-language runtime Vibecrafted.
- AICX może być niepełne albo wykonywane inną wersją niż checkout. Pusty wynik
  oznacza `unknown/no-attribution`, nie brak intencji.

Dlatego obecne twierdzenie brzmi **exposure aid**, nie predictor ani oracle.

## 12. Zamknięta kolejność preflightu Codescribe

1. **Ten dokument + journal** — utrwalić metodologię i role. To jest bieżący
   cut.
2. **P0-A** — przypisać i zakomitować wszystkie dirty surfaces; skończyć na
   czystym Living Tree.
3. **P0-B** — zakomitować publiczny, syntetyczny `five-Iwo` verifier, który jest
   RED na settled baseline z dokładnie oczekiwanego powodu.
4. **Dragon** — uruchomić ten sam verifier na dokładnie tym samym SHA i
   potwierdzić ten sam RED.
5. **W1-A/B/C** — dopiero wtedy uruchomić implementację ustawień, acoustic
   authority i delivery.
6. **W2-W5** — reducer, jeden autor tekstu, Swift projection i integrated
   acceptance.
7. Dopiero po W5: instalacja, żywy microphone walk-around i release gate.

Bieżący kontrakt repo wymaga jednego Living Tree. Stary Mode-B scaffold nadal
zawiera geometrię worktrees i musi zostać poprawiony przed dispatch. Koncepcyjna
równoległość W1 nie daje prawa do równoczesnych source writes w tym checkoutcie.

## 13. Instrukcja dla Dragona — niezależna replikacja, nie research

### Teraz

1. Przeczytaj ten handoff i wskazane artefakty R0.
2. Nie pisz kodu produktu.
3. Nie twórz własnego verifiera.
4. Nie uruchamiaj `five-Iwo`, dopóki integrator nie poda `P0_B_SHA` i dokładnej
   komendy.

### Po otrzymaniu P0_B_SHA

Na Dragonie:

```bash
cd /Users/polyversai/vc-workspace/vetcoders/codescribe
test -z "$(git status --porcelain)" || { git status --short; exit 70; }
git fetch origin
git switch --detach "$P0_B_SHA"
test "$(git rev-parse HEAD)" = "$P0_B_SHA"
loct context --full --markdown > /tmp/codescribe-five-iwo-loctree.md
bash scripts/verify-five-iwo.sh 2>&1 | tee /tmp/codescribe-five-iwo-red.log
```

Integrator musi przekazać SHA dostępny z origin albo osobny bezstratny bundle;
Dragon nie zgaduje brancha i nie testuje podobnego commita.

### Oczekiwany wynik baseline

- test collection jest niepusta;
- exit jest non-zero;
- failure wskazuje brak produkcyjnego przejścia occurrence → reducer → Bus →
  delivery, nie brak fixture, modelu, sekretu lub zależności;
- cztery regiony nie mogą zostać zinterpretowane jako pięć;
- fixture hash zgadza się z raportem P0-B.

Jeśli command przejdzie na zielono, padnie z innego powodu albo wymaga prywatnego
audio — **STOP**. To jest wadliwy RED lub nierówny substrate, nie pozwolenie na
implementację.

### Raport Dragona

Zapisz bez edycji kodu:

```text
/Users/polyversai/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/dragon-five-iwo-independent-red.md
```

Raport ma zawierać:

- `P0_B_SHA`, branch/detached state i `git status --porcelain`;
- `loct --version`, snapshot receipt i host;
- dokładną komendę, stdout/stderr i exit code;
- fixture SHA-256 i liczbę zebranych testów;
- oczekiwany failure mechanism kontra zaobserwowany;
- verdict `REPLICATED_RED`, `NON_EQUIVALENT_FAILURE` albo `SUBSTRATE_FAILURE`.

Dragon niczego nie poprawia. Wynik wraca do integratora.

## 14. Instrukcja dla Moniki — pełna ścieżka badawcza Loctree

**Granica dostępu:** żaden agent nie wchodzi na laptop Moniki bez jej osobnego,
jawnego zaproszenia. Maciej przekazuje tę kartę osobiście.

### Rola Moniki

Monika jest pełnoprawną badaczką Loctree prowadzącą lane Visty od receiptów,
przez mapę i klasyfikację kandydatów, po runtime trial i werdykt. Jej wiedza
domenowa nie zastępuje Loctree; jest potrzebna do prawidłowego opisania tego,
co Loctree znalazło i do odróżnienia legalnej projekcji od drugiego tronu.

Monika pracuje z Loctree aktywnym przez cały trial. Sama zachowuje raw outputy,
snapshot fingerprinty i false-positive/false-negative ledger. Zespół nie
„dołącza jej potem” technicznej prawdy — receipt strukturalny i prawda domenowa
powstają razem.

Przed próbą Monika definiuje:

1. czym produktowo jest jedna wizyta, pacjent, recording occurrence,
   transcript i entitlement;
2. które reprezentacje są legalnymi projekcjami;
3. które stany są niemożliwe albo niedopuszczalne;
4. jaki wynik danego scenariusza jest prawidłowy — zanim zobaczymy runtime;
5. czy dwa widoczne rekordy oznaczają dwa byty, dwie wersje jednego bytu czy
   błąd.

### Minimalny przebieg Loctree na obu repozytoriach Visty

Uruchomić osobno w rzeczywistym checkoutcie `vista` i `vista-portal`:

```bash
git status --short --branch
git rev-parse HEAD
loct --version
loct auto
loct context --full --markdown
loct repo-view
loct follow twins
loct follow pipelines
```

Następnie dla `vista`:

```bash
loct crowd visit
loct crowd patient
loct crowd transcript
loct crowd settings
loct focus src-tauri/src/commands/visits
loct focus src-tauri/src/vista_agent/tools
loct slice src-tauri/src/commands/visits/creation.rs
loct slice src-tauri/src/vista_agent/tools/visit_tools.rs
```

Loctree nie skleja dwóch repozytoriów Visty w jeden graf. Monika prowadzi więc
równolegle ręczny cross-repo edge ledger dla licensing/auth, zapisując source,
target, transport, identity, revision i kierunek authority. Brak importu między
repozytoriami nie jest brakiem zależności.

Wszystkie komendy i ich pełne outputy trafiają do katalogu artefaktów trialu,
nie tylko do terminal scrollback. Każdy miss Loctree trafia do append-only
`~/.vibecrafted/loctree/loctree-fail.md`.

### Pierwszy Vista trial

Pierwszy kandydat to jedna wizyta utworzona i zmieniona dwiema ścieżkami:

- kanoniczną komendą UI/Tauri;
- narzędziem agenta wykonującym własny `INSERT`/`UPDATE`.

Monika prerejestruje oczekiwania dla permission, patient identity, validation,
idempotency, version/CAS, eventu i stanu końcowego. Następnie sama mapuje w
Loctree write sites i ich relacje, a runtime probe wiąże je z DB rows i receipts.
Wynik klasyfikujemy według sekcji 10, nie według tego, czy pliki wyglądają
podobnie.

Drugim trialem jest entitlement: ten sam input portalu musi prowadzić do tego
samego fail-closed capability w portal resolverze, Rust ingress, TS derivation i
shell gate. Cache lub desktop validator nie może promować stanu po revoke.

### Karta, którą Monika wypełnia przed próbą

```text
TRIAL_ID:
BYT DOMENOWY:
KANONICZNA IDENTITY:
JEDEN OCZEKIWANY TRON:
LEGALNE PROJEKCJE:
STANY NIEDOZWOLONE:
SCENARIUSZ:
OCZEKIWANY WYNIK:
CO ODWRÓCI TEN WERDYKT:
```

Do karty Monika dołącza samodzielnie receipt repo/HEAD, Loctree snapshot,
writers, persistence, events, końcowy payload oraz `P(t)/A(t)/D(t)`. Badacz
techniczny może pomóc w runtime probe, ale nie przejmuje jej lane'u.

### Codescribe — obowiązkowa dodatnia kontrola metody

Codescribe nie jest opcjonalnym testem operatorskim. Jest obowiązkowym positive
control, na którym już znamy user-visible failure i rozproszenie kandydatów do
authority. Lane Visty porównuje precision/recall swoich instrumentów z tym
znanym przypadkiem:

1. ta sama sekwencja instrumentów Loctree na zamrożonym SHA Codescribe:
   `context → repo-view → focus → slice → literals → twins/prism`;
2. jawne wskazanie, które known thrones Loctree wystawiło, których nie wystawiło
   i jakie namesakes podało fałszywie;
3. porównanie authority union, owner transitions i kanałów poza AST z Vistą;
4. zapis false positives i false negatives przed zobaczeniem wyniku naprawy.

Po W5 osobny operatorski take może sprawdzić runtime conservation, ale nie
zastępuje strukturalnego positive control. Retranscribe nie jest kryterium
sukcesu live pipeline. Sukces oznacza zgodną liczność occurrence, reducer, Bus,
overlay i delivery bez ręcznego drugiego przebiegu.

## 15. Dostęp do naszych artefaktów przez SSH

Dragon i Monika mają czytać źródła z tego hosta (`div0`) przez istniejący,
autoryzowany dostęp SSH. Nie rozszerzamy POSIX permissions i nie kopiujemy
prywatnego audio do repo publicznego.

Przykładowe pobranie dokumentu na zaufanej maszynie:

```bash
scp div0:/Users/maciejgad/vc-workspace/vetcoders/codescribe/docs/LOCTREE_RESEARCH_TEAM_HANDOFF.md ./
```

Pobranie katalogu research bez kasowania lokalnych plików:

```bash
rsync -av --protect-args \
  div0:/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/ \
  ./codescribe-r0-research/
```

### Czytać w tej kolejności

1. Synteza integratora:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/2026-08-24_R0-integrator-synthesis_report.md`
2. Plan badawczy:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/plans/codescribe-one-throne-acoustic-authority-260824/research/R0_RESEARCH_PLAN.md`
3. Grok:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/2026-08-24_grok_plan-id-codescribe_report.md`
4. Claude:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/2026-08-24_claude_plan-id-codescribe_report.md`
5. Agy:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/2026-08-24_agy-gemini-3.7-flash-high_rsch-260824-024500-agy37_report.md`
6. Codex:
   `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/reports/research/2026-08-24_codex-gpt-5.6-sol_rsch-260824-025300-codex56c_report.md`

Uwaga: aktualny plik Claude ma 1 560 linii, ponieważ launcher później dopisał
kolejny przebieg do tej samej ścieżki. Synteza integratora dopuściła pierwotny,
zamknięty zakres 972 linii i dokumentuje tę provenance. Nie traktować dopisku
jako piątej niezależnej opinii.

### Scaffold i falsyfikatory

Plan root:

```text
/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/plans/codescribe-one-throne-acoustic-authority-260824
```

Najważniejsze pliki:

- `DRIVER.md` — dependency graph i komendy;
- `tracker.md` — stan cutów;
- `FALSIFICATION.md` — prawo pięciu Iwo i negative controls;
- `INSTRUMENTARIUM.md` — receipts i trial surfaces;
- `briefs/P0-01_substrate.md` — P0-A;
- `briefs/P0-02_five-iwo-verifier.md` — P0-B;
- `briefs/W1-*.md` do `W5-*.md` — późniejsze cuty;
- `mode-b.after-p0.dispatch.toml` — szablon, jeszcze nie gotowy do uruchomienia
  z powodu worktree driftu i braku P0-B SHA.

### Journal i dowody runtime

- append-only research journal:
  `/Users/maciejgad/vc-workspace/vetcoders/codescribe/.loctree/canary/JOURNAL.md`
- surowy append-only long evidence:
  `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/evidence_long.txt`
- zabezpieczony WAV:
  `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/evidence/audio/last_session_20260824T024536+0200_87f26667c1ec.wav`
- take pack z M4A i sumami:
  `/Users/maciejgad/.vibecrafted/artifacts/vetcoders/codescribe/2026_0824/evidence/takes/20260824T024537_w-obrebie-maszyny/`

Audio i `evidence_long.txt` są wewnętrznym materiałem badawczym. Nie trafiają do
Git, publicznego PR, publicznego issue ani zewnętrznego modelu bez osobnej zgody.

## 16. Stop-warunki

Zatrzymujemy cut i raportujemy jednoznacznie, gdy:

- RED wymaga produkcyjnej mutacji albo nowego testowego engine;
- cztery regiony mogą zostać zamienione w pięć z tekstu;
- ledger pada przed kontaktem z konkurentem — wtedy tron jest zły;
- tożsamość zależy od stringa, energii jako hash field lub sekund dekodera;
- nowy typ ma synchronizować dwa istniejące autorytety;
- Dragon nie może odtworzyć dokładnego SHA albo dostaje inny failure mechanism;
- runtime trial nie ma settings/env/log/Bus receipt;
- eksperyment Vista nie ma prerejestrowanej prawdy domenowej Moniki;
- ktoś chce promować Prism/twins do predykcyjnego verdictu bez replikacji i
  modelu błędu.

## 17. Następny baton

Po zapisaniu i zakomitowaniu tego handoffu integrator wykonuje wyłącznie:

1. P0-A na Living Tree;
2. P0-B jako publiczny syntetyczny RED;
3. przekazanie `P0_B_SHA` Dragonowi;
4. odbiór `dragon-five-iwo-independent-red.md`;
5. korektę starego Mode-B do bieżącego kontraktu Living Tree;
6. implementacyjny W1-A/B/C.

Nie ma pomiędzy tymi punktami kolejnego researchu Codescribe.
