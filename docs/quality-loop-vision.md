# Quality Loop Architecture: From Typos to Clinical Safety

> Specyfikacja wielopoziomowej architektury self-improvement dla ekosystemu CodeScribe/Vista.
>
> Created by M&K (c)2026 VetCoders

---

## Executive Summary

Quality loop to nie jest feature — to **architektura danych** która skaluje się od korekty literówek do audytu
bezpieczeństwa klinicznego. Ten sam wzorzec (raw → reference → delta → improvement) powtarza się na 4 poziomach, a każdy
poziom feeduje następny.

---

## 1. Architektura Wielopoziomowa

```mermaid
graph TD
    subgraph "POZIOM 1: Słowa (Lexicon)"
        L1_RAW["Whisper raw: 'klipy'"]
        L1_REF["Reference: 'clippy'"]
        L1_DELTA["Delta: lexicon entry"]
        L1_RAW --> L1_DELTA
        L1_REF --> L1_DELTA
        L1_DELTA --> L1_OUT["lexicon.custom.jsonl"]
    end

    subgraph "POZIOM 2: Transkrypcja (Fine-tune)"
        L2_RAW["WAV audio"]
        L2_REF["Corrected .txt (Claude/Human)"]
        L2_DELTA["Training pairs"]
        L2_RAW --> L2_DELTA
        L2_REF --> L2_DELTA
        L2_DELTA --> L2_OUT["whisper-small fine-tuned"]
    end

    subgraph "POZIOM 3: Notatki Kliniczne (SOAP)"
        L3_RAW["AI-generated SOAP draft"]
        L3_REF["Vet-reviewed SOAP"]
        L3_DELTA["Corrections as data"]
        L3_RAW --> L3_DELTA
        L3_REF --> L3_DELTA
        L3_DELTA --> L3_OUT["Better SOAP suggestions"]
    end

    subgraph "POZIOM 4: Safety Audit"
        L4_RAW["SOAP with dosages/drugs"]
        L4_REF["Clinically verified note"]
        L4_DELTA["Safety corrections"]
        L4_RAW --> L4_DELTA
        L4_REF --> L4_DELTA
        L4_DELTA --> L4_OUT["Safety-aware model"]
    end

    L1_OUT -->|feeds| L2_RAW
    L2_OUT -->|feeds| L3_RAW
    L3_OUT -->|feeds| L4_RAW
```

---

## 2. Poziom 1: Korekta Słów (DONE)

**Status:** Zaimplementowany w `codescribe-core/src/quality_loop.rs`

### Flow

```mermaid
flowchart LR
    A[Whisper raw output] --> B[StreamPostProcessor]
    B --> C{Lexicon match?}
    C -->|Yes| D[Apply correction]
    C -->|No| E[Pass through]
    D --> F[Corrected output]
    E --> F
    F --> G[Quality Report]
    G --> H[Loop: extract_lexicon_suggestions]
    H --> I[lexicon.custom.jsonl]
    I -->|hot-reload| B
```

### Dane wejściowe

- `~/.codescribe/transcriptions/<date>/*.wav` — audio
- `~/.codescribe/transcriptions/<date>/*.txt` — reference (ground truth)

### Dane wyjściowe

- `~/.codescribe/reports/quality_<ts>/report.json`
- `lexicon.custom.jsonl` — auto-generated corrections

### Reviewer: Claude (Klaudiusz)

- Zna domenę (weterynaria + programowanie)
- Zna akcent i wzorce mowy użytkownika
- Generuje reference .txt z raw Whisper output
- 95%+ accuracy bez ludzkiego odsłuchiwania
- Human wchodzi TYLKO na `[niewyraźne]` / `[niezrozumiałe]`

### Znane wzorce korekcyjne

| Whisper raw                    | Correct             | Domena               |
| ------------------------------ | ------------------- | -------------------- |
| klipy                          | clippy              | Rust tooling         |
| Locktri, logstri, Log3, LogTee | loctree             | Nasze narzędzie      |
| Alfaxon                        | Alfaksalon          | Anestezjologia wet.  |
| Robbena coxip                  | Robenacoxib         | NLPZ wet.            |
| SEMGREB                        | semgrep             | Security tooling     |
| kargotarpaulym                 | cargo-tarpaulin     | Rust coverage        |
| PNPM-12                        | pnpm dlx            | JS package manager   |
| exponential bugów              | exponential backoff | CS concept           |
| Pure Roost                     | Pure Rust           | Programming language |
| stadio                         | stdio               | Standard I/O         |
| CodeScrap                      | CodeScribe          | Nasz produkt         |

---

## 3. Poziom 2: Fine-tune Whisper Small

**Status:** Planowany (po ~10 cyklach loopa)

### Cel

v3-turbo-q8 (~888MB) jest za ciężki dla consumer Macs. Whisper-small fine-tuned na domain-specific data osiąga
porównywalną jakość **w naszej domenie** przy 5x mniejszym modelu.

### Flow

```mermaid
flowchart TD
    subgraph "Dragon (M3 Ultra)"
        A[v3-turbo-q8] --> B[Raw transcription]
        B --> C[Claude correction]
        C --> D["Pairs: WAV + reference.txt"]
    end

    subgraph "Fine-tuning"
        D --> E[Training dataset]
        E --> F[whisper-small fine-tune]
        F --> G[Domain-specific model]
    end

    subgraph "Deployment"
        G --> H[Vista @ Dr Darek]
        G --> I[Vista @ Karyna]
        G --> J[CodeScribe local]
    end
```

### Wymagania do fine-tune

- **Dane:** ~50-100h domain-specific audio + perfect references
- **Źródło:** 10+ cykli quality loopa × ~30 nagrań/cykl = 300+ par
- **Hardware:** Dragon (M3 Ultra, 512GB) — trening lokalny
- **Output:** whisper-small-pl-vetcoder (custom model)

### Przewaga

- Whisper-small: ~244MB vs v3-turbo: ~888MB
- Runs on M1 base, maybe iPhone (CoreML export)
- Domain-trained: zna "Alfaksalon", "loctree", "clippy" z factory

---

## 4. Poziom 3: SOAP Notes Quality

**Status:** Koncepcyjny

### SOAP = Subjective, Objective, Assessment, Plan

Standardowy format notatki klinicznej w weterynarii.

### Flow: Zamknięcie wizyty = Quality Gate

```mermaid
flowchart LR
    A[Transkrypcja wizyty] --> B[AI: Generate SOAP draft]
    B --> C[Keyword Highlighting]
    C --> D["Vet review (MANDATORY)"]
    D --> E{Corrections?}
    E -->|Yes| F[Delta: draft vs final]
    E -->|No| G[Approved as-is]
    F --> H[Training pair]
    G --> H
    H --> I[Better SOAP model]
    I --> B
```

### Mandatory Review = Automatic Training Data

**Kluczowy insight:** Zamknięcie wizyty w Vista WYMAGA przeglądu notatki. To nie jest opcjonalny krok — to jest obowiązkowa część workflow dokumentacji klinicznej.

To oznacza:

- **Każda zamknięta wizyta = verified training pair** (draft vs approved)
- **Keyword highlighting** (pomysł Moniki, kod istnieje!) podświetla leki, dawki, diagnozy — dokładnie to co wymaga uwagi
- **Workflow IS the loop** — żaden osobny "quality step" nie jest potrzebny
- **Skala:** 5 weterynarzy × 20 wizyt × 30 dni = **3000 par/miesiąc** bez dodatkowej pracy

### Transparentność: Vet jako Partner

> **"Wiesz, że każda Twoja korekta poprawia system dla wszystkich?"**

Vet **POWINIEN wiedzieć** że jego praca ulepszająca notatki ma wartość. To nie jest ukryte zbieranie danych — to partnerstwo:

- Vet widzi: "Twoje korekty poprawiły dokładność sugestii o 12% w tym miesiącu"
- Vet czuje: jestem współtwórcą lepszego narzędzia, nie betatesterem
- Vet zyskuje: z każdym miesiącem mniej korekt, bo system się uczy jego stylu
- **Motywacja:** Im dokładniej korektujesz, tym szybciej system przestaje wymagać korekt

To jest różnica między dark patternem a etycznym produktem.

### Analogia do Poziomu 1

| Quality Loop (L1)             | SOAP Loop (L3)                   |
| ----------------------------- | -------------------------------- |
| WAV = raw input               | Transcription = raw input        |
| Whisper output = hypothesis   | AI SOAP draft = hypothesis       |
| Reference .txt = ground truth | Vet-approved SOAP = ground truth |
| WER = metric                  | Clinical accuracy = metric       |
| Lexicon = correction tool     | SOAP templates = correction tool |
| Claude = reviewer             | Claude + Vet = reviewer          |
| Separate loop step            | **Built into workflow**          |

### Delta jako dane (z taksonomią)

Każda korekta veta w SOAP note to:

1. Co AI zaproponował (draft)
2. Co vet zmienił (final)
3. **Typ zmiany (delta type)** — kluczowe dla jakości uczenia
4. Kontekst (gatunek, wiek, masa, objawy)

→ Training data dla lepszych sugestii AI

### Taksonomia Delt (Delta Types)

> **Bez tego model miesza sygnał.** Korekta "Alfaksalon" → "Alfaxan" (marka vs generyk)
> to zupełnie inny sygnał niż "10mg/kg" → "3mg/kg" (safety critical).

```mermaid
graph TD
    DELTA[Delta: draft vs final] --> CLINICAL[Clinical]
    DELTA --> STYLISTIC[Stylistic]

    CLINICAL --> DOSAGE["DOSAGE — dawka leku (SAFETY)"]
    CLINICAL --> DRUG["DRUG — nazwa/zamiana leku (SAFETY)"]
    CLINICAL --> DIAGNOSIS["DIAGNOSIS — zmiana rozpoznania"]
    CLINICAL --> PLAN["PLAN — zmiana postępowania"]
    CLINICAL --> CONTRAINDICATION["CONTRAINDICATION — interakcja/przeciwwskazanie (SAFETY)"]

    STYLISTIC --> FORMAT["FORMAT — interpunkcja, skróty, układ"]
    STYLISTIC --> VERBOSITY["VERBOSITY — więcej/mniej szczegółów"]
    STYLISTIC --> TERMINOLOGY["TERMINOLOGY — synonim, nie zmiana merytoryczna"]
```

| Delta Type         | Waga w treningu | Przykład                                 |
| ------------------ | --------------- | ---------------------------------------- |
| `DOSAGE`           | **Krytyczna**   | "10mg/kg" → "3mg/kg"                     |
| `DRUG`             | **Krytyczna**   | "Meloksykam" → "Robenacoxib" (kot z CKD) |
| `CONTRAINDICATION` | **Krytyczna**   | dodanie "p/w u kotów z niewyd. nerek"    |
| `DIAGNOSIS`        | Wysoka          | "zapalenie" → "ropne zapalenie"          |
| `PLAN`             | Wysoka          | dodanie "kontrola za 3 dni"              |
| `TERMINOLOGY`      | Niska           | "Alfaksalon" → "Alfaxan" (marka)         |
| `FORMAT`           | Ignorowana      | przecinek, nowa linia                    |
| `VERBOSITY`        | Niska           | skrócenie opisu — styl veta              |

### Reference Quality Tags

> **Reference ≠ zawsze prawda.** Zaakceptowana notatka bywa kompromisem lub stylem.
> Bez tagów jakości nie wiemy czy uczymy się prawdy klinicznej czy nawyku.

Każda zaakceptowana notatka (reference) dostaje tag:

| Tag                  | Znaczenie                         | Wartość treningowa                  |
| -------------------- | --------------------------------- | ----------------------------------- |
| `CORRECTED_CLINICAL` | Vet zmienił lek/dawkę/diagnozę    | **Najwyższa** — uczenie safety      |
| `CORRECTED_STYLE`    | Vet zmienił styl/format           | Niska — personalizacja, nie klinika |
| `APPROVED_UNCHANGED` | Vet zaakceptował bez zmian        | Pozytywny sygnał — draft był OK     |
| `APPROVED_FAST`      | Vet kliknął <3s (może nie czytał) | **Pomijana** — niski confidence     |
| `OVERRIDDEN_SAFETY`  | Vet zignorował safety warning     | **Flagowana** — wymaga audytu       |

### Klasyfikacja Delta Type (reguły)

> **Zasada domyślna:** Jeśli delta dotyka tokenów high-risk → CLINICAL.
> Inaczej → STYLISTIC. Unikamy "szarej strefy".

```mermaid
flowchart TD
    A[Delta detected] --> B{Dotyka high-risk tokens?}
    B -->|"lek, dawka, mg/kg, i.v., s.c., p/w"| C[CLINICAL]
    B -->|"interpunkcja, układ, synonim"| D[STYLISTIC]
    C --> E{Zmiana wartości liczbowej?}
    E -->|Yes| F[DOSAGE]
    E -->|No| G{Zmiana nazwy leku?}
    G -->|Yes| H[DRUG]
    G -->|No| I{Dodanie przeciwwskazania?}
    I -->|Yes| J[CONTRAINDICATION]
    I -->|No| K[DIAGNOSIS / PLAN]
```

**High-risk tokens (seed list):**

- Jednostki: `mg`, `kg`, `ml`, `mg/kg`, `µg`, `j.m.`, `IU`
- Drogi podania: `i.v.`, `s.c.`, `i.m.`, `p.o.`, `per os`, `dożylnie`, `podskórnie`
- Leki: pattern match z bazy leków (vistakernel drug DB)
- Częstotliwość: `co 8h`, `BID`, `SID`, `TID`, `q12h`
- Kontraindykacje: `przeciwwskazane`, `p/w`, `nie stosować`, `uwaga`

### APPROVED_FAST — podwójna rola

`APPROVED_FAST` (<3s) to nie tylko "pomijana w treningu":

1. **Metryka adopcji UX** — ile notatek vet "przelatuje" = czy UI jest zbyt inwazyjny?
2. **Safety trigger** — jeśli w tekście wykryto high-risk tokens (leki/dawki) a vet kliknął <3s:
   - Vista wyświetla: "Notatka zawiera leki/dawki. Na pewno?"
   - Jeśli vet potwierdzi po ponownym przejrzeniu → tag zmienia się na `APPROVED_UNCHANGED`
   - Jeśli poprawi → `CORRECTED_CLINICAL` (wartościowe!)

### OVERRIDDEN_SAFETY — wartość analityczna

> **Override nie jest błędem veta** — to zdarzenie o wysokiej wartości analitycznej.

Może oznaczać:

- **Luka w KB** — safety warning był fałszywy bo baza nie zna wyjątku klinicznego
- **Wyjątek kliniczny** — vet wie coś czego model nie (np. specyficzny pacjent, toleruje wyższe dawki)
- **Nowy protokół** — vet stosuje nowszy schemat niż baza

Każdy override generuje:

1. Log z kontekstem (co było flagowane, co vet zostawił, dlaczego — opcjonalna nota)
2. Ticket do review KB (czy dodać wyjątek? czy update bazy?)
3. Sygnał do safety modelu: "ten pattern nie jest jednoznacznie błędny"

### Dwuścieżkowe uczenie

```mermaid
flowchart TD
    A[Delta] --> B{Delta type?}
    B -->|CLINICAL| C[Safety/Quality Model]
    B -->|STYLISTIC| D[Personalization Model]
    C --> E["Wspólny dla WSZYSTKICH instalacji"]
    D --> F["Lokalny per vet (styl pisania)"]
```

**Kluczowe:** Model safety/quality jest GLOBALNY — uczy się z klinicznych korekt wszystkich vetów.
Model personalizacji jest LOKALNY — uczy się stylu konkretnego veta (bez sensu pushować styl Darka do Karyny).

### Keyword Highlighting jako Safety + Collection

Istniejący kod (pomysł Moniki) podświetla kluczowe słowa przed akceptacją. Podwójna rola:

1. **Safety gate:** Vet zwraca uwagę na leki, dawki, diagnozy — łapie błędy
2. **Data focus:** Highlighted keywords = exact positions where corrections matter most
3. **Feedback signal:** Czy vet zmienił highlighted keyword? → wysoka wartość training signal

---

## 5. Poziom 4: Clinical Safety Audit

**Status:** Koncepcyjny (ale krytyczny z perspektywy odpowiedzialności)

### Cel

AI NIGDY nie może sugerować:

- Śmiertelnych dawek leków
- Kontraindykacji gatunkowych (np. NLPZ u kotów z niewydolnością nerek)
- Błędnych dróg podania
- Niezgodnych interakcji

### Flow

```mermaid
flowchart TD
    A[SOAP note with drugs/dosages] --> B[Safety Validator]
    B --> C{Risk detected?}
    C -->|"Alfaksalon 10mg/kg (3x za dużo)"| D[FLAG + suggest correct]
    C -->|OK| E[Pass through]
    D --> F[Vet confirms/overrides]
    F --> G[Safety delta]
    G --> H[Safety model training]
    H --> B

    subgraph "Safety Knowledge Base"
        I[Drug dosage ranges per species]
        J[Contraindications]
        K[Interaction matrix]
    end
    I --> B
    J --> B
    K --> B
```

### Odpowiedzialność producenta

- **Nie wymaga zgody użytkownika na dane** — to jest wewnętrzny QA produktu
- Safety model trenowany na danych VetCoders (własna wiedza kliniczna)
- Deploy do każdej instalacji Vista → chroni każdego veta
- Audyt trail: każda sugestia AI logowana z kontekstem

---

## 6. Federated Architecture (vistakernel)

```mermaid
graph TD
    subgraph "Libraxis Cloud"
        CLOUD_LEX[Cloud Lexicon]
        CLOUD_STT[Cloud STT API]
        CLOUD_MODELS[Model Registry]
        CLOUD_LEX --> CLOUD_STT
    end

    subgraph "Dragon (M&K Dev)"
        DEV_V3[v3-turbo-q8]
        DEV_LOOP[quality_loop]
        DEV_CLAUDE[Claude reviewer]
        DEV_V3 --> DEV_LOOP
        DEV_CLAUDE --> DEV_LOOP
        DEV_LOOP -->|lexicon updates| CLOUD_LEX
        DEV_LOOP -->|training pairs| CLOUD_MODELS
    end

    subgraph "Vista @ Dr Darek (Grudziądz)"
        DAREK_VK[vistakernel]
        DAREK_SMALL[whisper-small-finetuned]
        DAREK_LOOP[local loop]
        DAREK_VK --> DAREK_SMALL
        DAREK_VK --> DAREK_LOOP
        CLOUD_STT --> DAREK_VK
        CLOUD_MODELS -->|model update| DAREK_SMALL
    end

    subgraph "Vista @ Karyna (Raszyn)"
        KARYNA_VK[vistakernel]
        KARYNA_SMALL[whisper-small-finetuned]
        CLOUD_STT --> KARYNA_VK
        CLOUD_MODELS -->|model update| KARYNA_SMALL
    end

    subgraph "Opt-in Data Flow"
        DAREK_LOOP -.->|"za zgodą"| CLOUD_LEX
        KARYNA_VK -.->|"za zgodą"| CLOUD_LEX
    end
```

### Zasada: Improvement bez naruszenia prywatności

| Warstwa       | Bez zgody                     | Za zgodą           |
| ------------- | ----------------------------- | ------------------ |
| Cloud lexicon | Rośnie z pracy M&K            | + dane użytkownika |
| Model updates | Push do wszystkich instalacji | —                  |
| Safety audit  | Wewnętrzny QA VetCoders       | —                  |
| Local loop    | Działa autonomicznie          | Wyniki do chmury   |
| Training data | Tylko M&K pairs               | + użytkownicy      |

---

## 7. Roadmap

```mermaid
gantt
    title Quality Loop Evolution
    dateFormat YYYY-MM
    axisFormat %Y-%m

    section Poziom 1 (Lexicon)
    quality_report implementation     :done, l1a, 2026-01, 2026-01
    quality_loop implementation       :done, l1b, 2026-01, 2026-01
    Claude as reviewer (auto-ref)     :active, l1c, 2026-01, 2026-02
    Lexicon hot-reload                :done, l1d, 2026-01, 2026-01

    section Poziom 2 (Fine-tune)
    Accumulate 10+ loop cycles        :l2a, 2026-02, 2026-04
    Prepare training dataset          :l2b, 2026-04, 2026-05
    Fine-tune whisper-small           :l2c, 2026-05, 2026-06
    Deploy to Vista installations     :l2d, 2026-06, 2026-07

    section vistakernel
    codescribe-core → vistakernel     :active, vk1, 2026-01, 2026-03
    Local loop in Vista               :vk2, 2026-03, 2026-04
    Cloud lexicon sync                :vk3, 2026-03, 2026-05

    section Poziom 3 (SOAP)
    SOAP generation in Vista          :l3a, 2026-04, 2026-06
    SOAP quality loop                 :l3b, 2026-06, 2026-08
    Vet feedback collection           :l3c, 2026-06, 2026-09

    section Poziom 4 (Safety)
    Drug dosage knowledge base        :l4a, 2026-06, 2026-08
    Safety validator prototype        :l4b, 2026-08, 2026-10
    Safety audit loop                 :l4c, 2026-10, 2026-12
```

---

## 8. Kluczowe Decyzje Architektoniczne

### 8.1 Claude jako reviewer (Poziom 1-2)

- **Przewaga:** Zna domenę, akcent, kontekst lepiej niż generic AI
- **Ograniczenie:** Nie słyszy audio — operuje na raw text
- **Human fallback:** Tylko `[niewyraźne]` / `[niezrozumiałe]`
- **Skalowanie:** Automatyczny, batch, bez bottlenecka

### 8.2 vistakernel jako portable engine

- **Pochodzenie:** codescribe-core (Rust, portable, no macOS deps)
- **Deploys to:** macOS (CodeScribe), Vista (cross-platform), potentially iOS
- **Contains:** Whisper engine, StreamPostProcessor, quality loop, lexicon
- **Does NOT contain:** UI, hotkeys, tray, clipboard — platform-specific

### 8.3 Federated vs Centralized

- **Federated (default):** Każda instalacja Vista ma lokalny loop
- **Centralized (opt-in):** Dane użytkowników feedują cloud lexicon
- **Hybrid:** Cloud models pushowane do wszystkich, dane zbierane za zgodą

### 8.4 WER nie jest jedyną metryką

- **Poziom 1-2:** WER/CER (word/character error rate) — dobra metryka
- **Poziom 3:** Clinical accuracy (terminology, completeness, structure)
- **Poziom 4:** Safety score (false negatives = missed dangerous suggestions)
- **Cross-level:** Semantic similarity (BGE-M3 embeddings) — already in StreamPostProcessor

---

## 9. Istniejąca Implementacja (Stan na 2026-01-23)

### Pliki w repo

| Plik                                        | LOC   | Rola                                               |
| ------------------------------------------- | ----- | -------------------------------------------------- |
| `codescribe-core/src/quality_report.rs`     | 1563  | Batch: WAV → transcripts → metrics → report        |
| `codescribe-core/src/quality_loop.rs`       | ~1200 | Analyze report → regressions → lexicon suggestions |
| `codescribe-core/src/stream_postprocess.rs` | 663   | Runtime lexicon correction + semantic gate         |
| `src/bin/codescribe_loop.rs`                | 561   | CLI: single run / daemon mode                      |
| `src/bin/codescribe_quality.rs`             | ~200  | CLI: quality report only                           |
| `assets/programming.jsonl`                  | —     | Built-in programming lexicon                       |
| `assets/veterinary.jsonl`                   | —     | Built-in veterinary lexicon                        |

### Komendy

```bash
# Generate quality report
codescribe quality --input ~/.codescribe/transcriptions --date 2026-01-17

# Run loop (single iteration)
codescribe loop --input ~/.codescribe/transcriptions --apply

# Run loop daemon (background, periodic)
codescribe loop --daemon --interval 3600 --apply
```

### Metryki w report.json

- `avg_raw_wer` — Whisper bez korekcji
- `avg_post_wer` — po StreamPostProcessor (lexicon)
- `avg_ai_wer` — po AI formatting (LLM)
- `avg_cloud_wer` — cloud STT (Libraxis API)

---

_Created by M&K (c)2026 VetCoders_
_Klaudiusz & Maciej — from SyntaxError to Clinical Safety_
