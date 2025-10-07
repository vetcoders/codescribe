# VistaScribe — Kroki dla Moniki i Bartka (wewnętrzne)

## Opcja A — instalacja z DMG (najprostsza)
1) Skopiuj plik `packaging/dmg/VistaScribe.dmg` na swój Mac i otwórz.
2) W oknie DMG:
   - Helpers → `Get Models.command` (pobierze Whisper — wybierz Large v3 Turbo lub Medium).
   - Helpers → `Install App.command` (skopiuje do `/Applications` i uruchomi).
3) Przy pierwszym uruchomieniu nadaj uprawnienia macOS (System Settings → Privacy & Security):
   - Microphone (Terminal/Python)
   - Accessibility (Terminal/Python)
   - Input Monitoring (Terminal/Python)
4) Użycie:
   - Przytrzymaj `Ctrl` ≥ 500 ms → nagrywanie → puszczasz → wkleja tekst.
   - Dwuklik `Option (⌥)` → tryb toggle.

## Opcja B — uruchomienie z repo (dev)
1) Skopiuj repo (`VistaScribe`) i przejdź do katalogu głównego.
2) Zainstaluj zależności:
   ```bash
   uv sync
   ```
3) Pobierz modele (lub skopiuj je do `./models`):
   ```bash
   uv run python scripts/get_models.py --whisper large-v3-turbo
   ```
4) Start w tle (tray + backend):
   ```bash
   ./scripts/quickstart_mac.sh --mode both --daemon --log VistaScribe.log
   ```
   - Log podgląd: `tail -f VistaScribe.log`
   - Zatrzymanie: `./scripts/quickstart_mac.sh --stop-all`

## Skróty i ustawienia
- Domyślnie aktywny jest **Light Plus** (FORMAT_STRATEGY=light_plus) — szybkie i bez modelu LLM.
- W tray → Hotkey Settings: zmiana kombinacji (Ctrl / Ctrl+Option / Ctrl+Shift / Ctrl+Command) i tryb Exclusive (Ctrl nie działa z innymi modyfikatorami). Zapis do `.env` robi się automatycznie.
- Tray → Feedback: dźwięk startu (Tink/Pop) + głośność. Też zapisuje do `.env`.

## Gdzie są logi i PIDy
- Tray (wrapper): `~/Library/Logs/VistaScribe.app.log`
- Z quickstarta: `VistaScribe.log`, backend: `logs/backend.*.log`
- Pliki PID (do awaryjnego kill): `.pids/tray.pid`, `.pids/backend.pid`

## Wyłącz formatowanie albo użyj modelu LLM
- Wyłączyć: tray → Disable “Enable Formatting” lub `FORMAT_ENABLED=0` w `.env`.
- Mały LLM (opcjonalnie): ustaw `LLM_ID=/ścieżka/do/qwen-4b` i `FORMAT_STRATEGY=llm`.

## Rozwiązywanie problemów
- Brak wklejania/skrótów: sprawdź uprawnienia Accessibility/Input Monitoring.
- Brak dźwięku: Feedback → Enable Start Sound, SOUND_VOLUME 0.1–0.3.
- Modele nie widoczne: skopiuj do `./models` albo uruchom `Get Models.command`.
- Reset: `./scripts/quickstart_mac.sh --stop-all` i start ponownie.

