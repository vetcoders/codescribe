# 𝒱𝒾𝓈𝓉𝒶𝒮𝒸𝓇𝒾𝒷ℯ
<p align="center">
  <img src="assets/icon.png" alt="𝒱𝒾𝓈𝓉𝒶𝒮𝒸𝓇𝒾𝒷ℯ" width="128" height="128">
</p>

<h3 align="center">"Mów, a ja zapiszę."</h3>

---

## O co w tym wszystkim chodzi?

**𝒱𝒾𝓈𝓉𝒶𝒮𝒸𝓇𝒾𝒷ℯ** to aplikacja do transkrypcji mowy na tekst dla macOS, która żyje sobie dyskretnie w Twoim pasku menu. Nagrywa dźwięk po naciśnięciu globalnego skrótu klawiszowego, przetwarza go lokalnie (żadnych wścibskich chmur!) i wkleja wynik tam, gdzie akurat masz kursor.

Koniec z API, kluczami. Chyba że chcesz — wtedy też się da.

## Główne atrakcje

-   **Lokalna Transkrypcja:** Używa [MLX Whisper](https://huggingface.co/mlx-community/whisper-large-v3-turbo) na Twoim Macu. Twoje dane zostają Twoje.
-   **Globalne Skróty Klawiszowe:**
    -   **Naciśnij i przytrzymaj `Control`:** Nagrywaj, póki trzymasz. Proste.
    -   **Podwójne stuknięcie `Option (⌥)`:** Rozpoczyna i kończy nagrywanie w dowolnym miejscu.
-   **Automatyczne Wklejanie:** Po transkrypcji tekst ląduje w schowku i jest od razu wklejany. Magia! (A tak naprawdę to symulacja `⌘V`).
-   **Lokalne formatowanie (opcjonalne):** Jeśli surowy tekst z Whisper to dla Ciebie za mało, lokalny model językowy może dodać kropki i wielkie litery.
-   **Dyskretny Interfejs:** Ikona w pasku menu informuje, co się dzieje (🜏 Czuwam, ◉ Nagrywam, … Myślę, ✓ Zrobione).

## Instalacja dla "zwykłych ludzi" (.dmg)

Jeśli nie chcesz grzebać w kodzie, to jest droga dla Ciebie.

1.  **Pobierz modele:** Kliknij `Helpers/Get Models.command`. To ściągnie modele Whisper. Bez tego cała zabawa nie ma sensu.
2.  **Zainstaluj backend:** Kliknij `Helpers/Install Backend.command`. To zainstaluje i uruchomi serwer w tle, który będzie czekał na Twoje polecenia.
3.  **Uruchom aplikację:** Przeciągnij `Vista Scribe.app` do folderu `Aplikacje` i uruchom.
4.  **Udziel pozwoleń:** macOS zapyta o dostęp do mikrofonu, ułatwień dostępu i monitorowania wejścia. Zgódź się, inaczej aplikacja będzie bezużyteczna.
5.  **Używaj:** Postaw kursor w dowolnym polu tekstowym, stuknij dwa razy `Option (⌥)` i zacznij mówić.

## Instalacja dla deweloperów (i masochistów)

Czujesz się odważny? Proszę bardzo.

### Wymagania

-   macOS
-   Python 3.9+
-   `uv` (bo kto ma czas na `venv` i `pip` osobno?)

### Kroki

1.  **Sklonuj repozytorium:**
    ```bash
    git clone https://github.com/LibraxisAI/VistaScribe.git
    cd VistaScribe
    ```

2.  **Zainstaluj zależności:**
    ```bash
    uv sync
    ```

3.  **Pobierz modele:**
    ```bash
    uv run python scripts/get_models.py --whisper large-v3-turbo
    ```

4.  **Uruchom aplikację:**
    ```bash
    uv run python main.py
    ```
    Lub jeśli wolisz wersję serwerową:
    ```bash
    uv run python backend.py
    ```

## Budowanie paczki `.dmg`

Chcesz podzielić się swoją pracą z innymi? Zbudujmy instalator.

```bash
# Najpierw opcjonalnie zbuduj aplikację .app
(cd packaging && python setup.py py2app)

# Potem zbuduj obraz .dmg
sh packaging/dmg/build_dmg.sh

# Otwórz i podziwiaj swoje dzieło
open packaging/dmg/VistaScribe.dmg
```

**Uwaga:** Budowanie `.app` z `py2app` bywa kapryśne, zwłaszcza z bibliotekami takimi jak `PortAudio`. Jeśli coś pójdzie nie tak, nie mów, że nie ostrzegałem.

## Licencja

MIT. Rób z tym, co chcesz, ale jeśli coś zepsujesz, to Twój problem.

