# CodeScribe Voice & Chat Lab

**URL**: `http://127.0.0.1:8237/lab`

## Zakładki

| Tab | Opis |
|-----|------|
| **Voice Lab** | Spektrogram + streaming WebSocket + testy endpointów |
| **Chat** | Testowy chat z AI (Harmony/OpenAI) |
| **Teacher** | Aktywne uczenie się - kalibracja słownika |

---

## 🎓 Teacher - Instrukcja obsługi

### Cel
Whisper źle transkrybuje specjalistyczne słowa? Teacher pozwala nauczyć go poprawnej wymowy przez porównanie tego co **powiedziałeś** z tym co **usłyszał**.

### Flow w 5 krokach

```
1. Wybierz kategorię (topic)
        ↓
2. AI generuje zdania do przeczytania
        ↓
3. Czytasz zdanie na głos (nagrywasz)
        ↓
4. Poprawiasz transkrypt jeśli Whisper się pomylił
        ↓
5. Klikasz "Learn" → różnice trafiają do słownika
```

### Szczegółowy przepis

#### Krok 1: Wybierz kategorię
Wpisz temat w pole **Topic**, np:
- `veterinary` - terminologia weterynaryjna
- `programming` - nazwy funkcji, bibliotek
- `liturgia` - terminy kościelne
- `cooking` - kulinaria

#### Krok 2: Wygeneruj zdania
Kliknij jeden z przycisków:
- **Generate Set** - 5 zdań do ręcznego wyboru
- **Wizard (10)** - tryb krok-po-kroku, 10 zdań

#### Krok 3: Nagraj wymowę
1. Przejdź do zakładki **Voice Lab**
2. Kliknij **Start Stream** (mikrofon)
3. Przeczytaj zdanie wyświetlone w Reference
4. Kliknij **Stop Stream**
5. Wróć do **Teacher**

#### Krok 4: Porównaj i popraw
- **Reference Text** = co powiedziałeś (oryginalne zdanie)
- **Transcript** = co Whisper usłyszał

Jeśli Whisper źle zrozumiał, **popraw Transcript** ręcznie na to co naprawdę mówiłeś.

#### Krok 5: Zapisz do słownika
Kliknij:
- **🧠 Fix & Learn** - zapisz różnice i zostań
- **Learn & Next ▶** - zapisz i przejdź do następnego zdania (Wizard)

---

## Gdzie trafiają poprawki?

Różnice zapisują się do pliku lexicon w `~/.CodeScribe/lexicon/{topic}.jsonl`:

```json
{"term": "ketoprofen", "mispronunciations": ["keto profen", "ketoprafen"]}
{"term": "amoksycylina", "mispronunciations": ["amoksycyliny", "amoxicilina"]}
```

Te reguły są używane przez Light Plus do poprawiania transkrypcji w locie.

---

## Przyciski w Teacher

| Przycisk | Funkcja |
|----------|---------|
| **Generate Set** | Wygeneruj 5 zdań AI |
| **Wizard (10)** | Tryb krok-po-kroku (10 zdań) |
| **Preview** | Podgląd aktualnego słownika |
| **Reload** | Przeładuj słownik z dysku |
| **Clear** | Wyczyść słownik dla tematu |
| **Export** | Eksportuj słownik do JSON |
| **📋** (przy zdaniu) | Skopiuj zdanie do Reference |
| **Next ▶** | Następne zdanie w Wizard |
| **⬇ Pull Last Transcript** | Pobierz ostatni transkrypt |

---

## Metryki

Po każdym Learn widzisz:
- **WER** (Word Error Rate) - % błędnych słów
- **Avg WER** - średnia ze wszystkich prób
- **Export report** - zapisz całą sesję kalibracji

---

## Troubleshooting

| Problem | Rozwiązanie |
|---------|-------------|
| Brak transkryptu | Upewnij się że kliknąłeś Start Stream w Voice Lab |
| "Generate failed" | Sprawdź czy backend ma dostęp do AI (Harmony/OpenAI) |
| Słownik pusty | Kliknij Reload lub sprawdź `~/.CodeScribe/lexicon/` |

---

Created by M&K (c)2025 The LibraxisAI Team
