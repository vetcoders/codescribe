# VistaScribe-Py

Local, private speech-to-text for macOS. VistaScribe runs in the menu bar, records on a global hotkey, transcribes with local MLX Whisper, optionally formats with a local MLX-LM, and pastes the result wherever your cursor is. No API keys required (cloud backends still available as optional fallbacks).

For a full project overview, see docs/PROJECT_DESCRIPTION.md.

## Błyskawiczny start (macOS, lokalnie, bez kluczy)

Najprostsza droga bez grzebania w konfiguracji — 3 kroki:

1) Zainstaluj uv (jeśli nie masz):

-   **Menu Bar Interface:** Runs discreetly in the macOS menu bar.
-   **Hotkey Activation:**
    -   **Hold Mode:** Hold the `Control` key to record.
    -   **Toggle Mode:** Press `Shift + Command + /` (⇧⌘/) to start/stop recording.
    -   **New (Local):** Double‑tap the Option (⌥) key to start/stop recording anywhere (global). Default double‑tap window: 350 ms (tunable via `DOUBLE_OPTION_INTERVAL_MS`).
-   **OpenAI Integration:**
    -   Uses Whisper API for transcription.
    -   Uses GPT-4o-mini API for punctuation and capitalization cleanup.
-   **Automatic Pasting:** Copies the final text to the clipboard and simulates `Command + V` (⌘V).
-   **Status Indicators:** Menu bar icon changes to show status (Idle: 🜏, Recording: ◉, Processing: …, Success: ✓).
-   **Silence Detection:** Automatically stops recording after a short period of silence (currently ~0.8 seconds).

## Installation (.dmg) — VistaScribe

This is the simplest way for non‑developers to get started.

What’s inside the DMG:

-   Vista Scribe.app (menu‑bar tray app; optional if you only want Finder Quick Action + backend)
-   Helpers/Install Backend.command (sets up and starts the background server)
-   Helpers/Get Models.command (downloads Whisper locally; LLM optional)
-   Helpers/Uninstall Backend.command (stops and removes the background server)
-   Extras/com.VistaScribe.backend.plist (template for reference)

How to install (from DMG):

1.  Drag “Vista Scribe.app” into /Applications (optional but recommended).
2.  Double‑click Helpers/Get Models.command — downloads Whisper to ./models. If you want the smaller Whisper, export WHISPER_VARIANT=medium before running.
3.  Double‑click Helpers/Install Backend.command — installs a LaunchAgent that keeps the local backend running (Whisper + optional LLM) at login.
4.  First run: macOS will ask for permissions (Microphone, Accessibility, Input Monitoring). Approve them for your Terminal (for first run) and for Vista Scribe.app when you launch it.
5.  Test backend: `curl -s http://127.0.0.1:8237/healthz | jq` should return ok.
6.  Use it: put the cursor in any text field, double‑tap Option (⌥) to dictate; text is pasted automatically.

Notes:

-   LLM formatting is optional. To disable: re‑run Install Backend with `FORMAT_ENABLED=0` (or edit the LaunchAgent plist in `~/Library/LaunchAgents/com.VistaScribe.backend.plist`).
-   To pick a local LLM, set `LLM_ID=/users/…/models/<your-mlx-llm>` before running Install Backend, or edit the plist later (use lowercase `/users` on macOS when possible).
-   To uninstall the backend: double‑click Helpers/Uninstall Backend.command.

Build the DMG yourself (for developers):

```bash
# Optionally build the .app tray bundle first (has known PortAudio bundling caveats)
(cd packaging && python setup.py py2app)
# Build DMG (includes helper scripts; will include the app if present)
sh packaging/dmg/build_dmg.sh
open packaging/dmg/VistaScribe.dmg
```

Troubleshooting:

-   If MLX refuses paths with uppercase in `/Users`, prefer `/users` variants (we auto‑normalize where possible).
-   If the app can’t access the mic or paste, grant permissions in System Settings → Privacy & Security (Microphone, Accessibility, Input Monitoring).

## Setup

### Prerequisites

-   macOS
-   [Homebrew](https://brew.sh/) (for installing Python)
-   Python 3.9+
-   An OpenAI API Key

### Installation Steps

1.  **Clone the Repository:**
    
    ```bash
    git clone https://github.com/AlexHagemeister/VistaScribe.git
    cd VistaScribe
    ```
    
2.  **Install Python (if needed):**
    
    ```bash
    brew install python
    ```
    
3.  **Create Virtual Environment:**
    
    ```bash
    python3 -m venv .venv
    ```
    
4.  **Activate Environment:**
    
    ```bash
    source .venv/bin/activate
    ```
    
    *(Your terminal prompt should now start with `(.venv)`)*
    
5.  **Install Dependencies:**
    
    ```bash
    pip install --upgrade pip
    pip install -r requirements.txt
    ```
    
6.  **Configure API Key:**
    
    -   Create a file named `.env` in the project root directory.
    -   Add your OpenAI API key to it:
        
        ```env
        OPENAI_API_KEY="sk-YOUR_API_KEY_HERE"
        ```
        
    -   *(Note: The `.env` file is included in `.gitignore` to prevent accidentally committing your key.)*

## Usage (Running from Source)

1.  **Activate Environment:**
    
    ```bash
    source .venv/bin/activate
    ```
    
2.  **Run the Application:**
    
    ```bash
    python main.py
    ```
    
3.  **Grant Permissions (First Run):** macOS will prompt you to grant permissions:
    
    -   **Microphone Access:** Needed for recording audio.
    -   **Accessibility Access:** Needed for simulating the paste command (⌘V).
    -   **Input Monitoring:** Needed to detect the global hotkeys (Ctrl, ⇧⌘/). *You may need to manually enable these for your Terminal application or Python itself in **System Settings > Privacy & Security**.*
4.  **Transcribe:**
    
    -   Click into any text field where you want to paste text.
    -   **Hold Mode:** Press and hold the `Control` key, speak your phrase clearly, and release the key.
    -   **Toggle Mode:** Press `Shift + Command + /`, speak your phrase, and press `Shift + Command + /` again.
    -   The menu bar icon will indicate the status (◉ → … → ✓).
    -   The transcribed and formatted text should automatically paste into the active field.
5.  **Quit:** Click the menu bar icon (🜏) and select "Quit".
    

## Packaging (`.app` Bundle)

*(Note: See 'Current Status' below regarding build issues.)*

1.  **Activate Environment:**
    
    ```bash
    source .venv/bin/activate
    ```
    
2.  **Navigate to Packaging Directory:**
    
    ```bash
    cd packaging
    ```
    
3.  **Run py2app:**
    
    ```bash
    python setup.py py2app
    ```
    
4.  **Find the App:** The `VistaScribe.app` bundle will be created in the `packaging/dist/` directory.
    
5.  **Install & Run:**
    
    -   Drag `VistaScribe.app` to your `/Applications` folder.
    -   Double-click the app in `/Applications` to run it.
    -   Grant Microphone, Accessibility, and Input Monitoring permissions again, this time specifically for `VistaScribe.app`.

## Launch at Login

*(This requires a working `.app` bundle placed in `/Applications`)*

1.  **Copy LaunchAgent Plist:**
    
    ```bash
    cp packaging/com.VistaScribe.plist ~/Library/LaunchAgents/
    ```
    
2.  **Load LaunchAgent:**
    
    ```bash
    launchctl load ~/Library/LaunchAgents/com.VistaScribe.plist
    ```
    

VistaScribe should now launch automatically the next time you log in. To unload it:

```bash
launchctl unload ~/Library/LaunchAgents/com.VistaScribe.plist
rm ~/Library/LaunchAgents/com.VistaScribe.plist
```

## Current Status & Known Issues

-   **Core Functionality:** The application runs correctly when launched from source code via `python main.py`. Hotkeys, recording, transcription, formatting, and pasting are functional.
-   **`.app` Build:** The application bundle created by `py2app` currently **fails to launch**. Debugging indicates an issue with packaging or finding the `libportaudio.dylib` library required by the `sounddevice` package within the bundled environment (`OSError: PortAudio library not found`). Attempts to fix this by explicitly including `sounddevice` in `setup.py` packages have not yet resolved the launch error. Further investigation into `py2app` configuration or potential workarounds (like manually including the dylib) is needed.

## License

MIT License (See LICENSE file for details)

## Benchmarking local Bielik models (PnC formatting)

You can compare three local MLX-LM models (Bielik 1.5B, 4.5B, 11B) on a Polish formatting task (punctuation + capitalization + minor typo handling) using the included script.

Notes:

-   If you use absolute paths on macOS, MLX can reject uppercase in `/Users`. Prefer lowercase `/users` (the script attempts to normalize automatically).
-   Results are saved to `outputs/bench/format_benchmark.{md,json}`.

Run (defaults assume models live in `./models/`):

```bash
uv run python format_benchmark.py
```

Override model locations (recommended to use lowercase `/users/...` if absolute):

```bash
MODEL_1P5_PATH="/users/you/hosted/vistas/VistaScribe/models/bielik-1.5b-mxfp4-mlx" 
MODEL_4P5_PATH="/users/you/hosted/vistas/VistaScribe/models/bielik-4.5b-mxfp4-mlx" 
MODEL_11B_PATH="/users/you/hosted/vistas/VistaScribe/models/bielik-11b-mxfp4-mlx" 
uv run python format_benchmark.py
```

Customize the test text:

```bash
BENCH_TEXT="to jest testowy transkrypt bez kropek i z literowkami itd" 
uv run python format_benchmark.py
```

Generation parameters can be adjusted via env vars (defaults shown):

-   `TEMPERATURE=0.2`
-   `TOP_P=0.0`
-   `TOP_K=0`
-   `MAX_NEW_TOKENS=128`

---

## Local models quick start (MLX, no API key)

After cloning, use the helper to download models locally into ./models (choose Whisper variant; LLM is optional):

```bash
uv run python scripts/get_models.py --whisper large-v3-turbo
# or
uv run python scripts/get_models.py --whisper medium
# or download both whisper variants + an optional LLM
uv run python scripts/get_models.py --whisper all 
  --llm mlx-community/Llama-3.2-3B-Instruct-4bit
```

Select which Whisper to use at runtime (pick one):

```bash
# Option A: by variant
export WHISPER_VARIANT=large-v3-turbo   # or: medium

# Option B: by explicit path (recommended to prefer lowercase /users if available on macOS)
export WHISPER_DIR=/users/you/hosted/vistas/VistaScribe/models/whisper-large-v3-turbo
```

LLM formatting is optional. Disable it entirely or point to your local LLM path/HF repo id:

```bash
# Disable formatting (paste raw Whisper transcript)
export FORMAT_ENABLED=0

# OR enable local formatting (example)
export FORMAT_ENABLED=1
export LLM_ID=/users/you/hosted/vistas/VistaScribe/models/bielik-4.5b-mxfp4-mlx
# (alternatively) use a HF MLX repo id and it will auto-download on first use
# export LLM_ID=mlx-community/Llama-3.2-3B-Instruct-4bit
```

Run the app:

```bash
uv run python main.py
```

Tip (macOS path quirk): If absolute paths with '/Users/…' cause issues in MLX, prefer the lowercase '/users/…' variant when it exists on your system.

---

## VistaScribe backend (FastAPI) + LaunchAgent + Quick Action (Q2)

This repository now includes a local backend server you can run in the background (no API keys). It keeps Whisper + LLM in memory for instant responses and enables a Finder Quick Action that calls the backend over HTTP (recommended Q2 setup).

### Start the backend (manual run)

```bash
# Pick your models (remember MLX path quirk: prefer lowercase /users if using absolute paths)
export WHISPER_DIR=./models/whisper-large-v3-turbo   # or set WHISPER_VARIANT=medium
export LLM_ID=/users/you/hosted/vistas/VistaScribe/models/bielik-4.5b-mxfp4-mlx
export FORMAT_ENABLED=1

# Run the server
uv run python backend.py
```

Healthcheck:

```bash
curl -s http://127.0.0.1:8237/healthz | jq
```

Endpoints:

-   POST /transcribe (multipart audio)
-   POST /format (json {text, instruction?})
-   POST /stt_and_format (multipart audio + optional instruction)
-   POST /action (json {action}) — updates backend state for widgets (listening/idle/muted/etc.)
-   GET /events (text/event-stream) — SSE stream of {state}

### Auto-start at login (LaunchAgent)

```bash
# Copy the prepared plist
mkdir -p ~/Library/LaunchAgents
cp packaging/launchagents/com.VistaScribe.backend.plist ~/Library/LaunchAgents/

# Load and start
launchctl load ~/Library/LaunchAgents/com.VistaScribe.backend.plist
launchctl start com.VistaScribe.backend

# Logs
tail -f /tmp/VistaScribe.backend.err.log
```

Edit the plist if your paths differ — use /users/... for MLX model paths when possible.

### Finder Quick Action (Q2: calls backend HTTP)

1.  Open macOS “Automator” → new “Quick Action”.
2.  Set “Workflow receives current: files or folders” in “Finder”.
3.  Add “Run Shell Script”. Shell = /bin/zsh. “Pass input: as arguments”.
4.  Script content:

```bash
/users/you/hosted/vistas/VistaScribe/scripts/quick_action_backend.sh "$@"
```

5.  Save as “Transkrybuj (VistaScribe)”.

Usage: Right‑click any audio file in Finder → Quick Actions → “Transkrybuj (VistaScribe)”. The result is saved next to the file as .transkrypcja.txt, copied to clipboard, and a macOS notification appears.

Notes:

-   Ensure the backend is running (LaunchAgent or manual run).
-   LLM formatting is optional; set FORMAT_ENABLED=0 to paste raw Whisper output.
-   Default generation: T=0.2, MAX_NEW_TOKENS=128 (tunable via env).

---

## Tray icon (menu bar)

Domyślnie aplikacja używa ikony z repo: assets/icon.png (bez żadnych exportów). Możesz opcjonalnie wskazać własny plik PNG/ICNS przez TRAY_ICON.

- Domyślna ścieżka: assets/icon.png
- Opcjonalna zmiana:

```bash
export TRAY_ICON="/path/to/your/icon.png"
uv run python main.py
```

Uwagi:
- Na macOS normalizujemy "/Users" do "/users" jeśli taka ścieżka istnieje.
- Gdy ikona jest ustawiona, VistaScribe ukrywa tekstowy tytuł obok ikony w pasku menu.

## Ruff lint & format

Ruff is configured for this repo and used for both formatting and linting.

Run locally without installing anything globally (via uvx):

```bash
uvx ruff format .
uvx ruff check .
```

Or with the project environment (after `uv sync`):

```bash
uv run ruff format .
uv run ruff check .
```

Configuration lives in `pyproject.toml` under `[tool.ruff]`. The default line-length is 100 and lint
rules are tuned to be pragmatic for this project. Hotkey debug prints are allowed for now.

## CI: lint

A lightweight GitHub Actions workflow runs Ruff on every push/PR (`.github/workflows/lint.yml`). It
uses `uvx ruff` so it does not need to install the full project dependencies. Tests can still be
run locally with `uv run pytest -q`.

## Acknowledgements

Initially based on [whisprflow-clone](https://github.com/AlexHagemeister/whisprflow-clone.git)


---

## FAQ: Jak ustawiać zmienne środowiskowe (zsh/bash)

Jeśli wpiszesz tylko same przypisania:

```
FORMAT_ENABLED=1 WHISPER_VARIANT=large-v3-turbo
```

…to nic się nie wydarzy (to nie jest komenda). Użyj jednej z dwóch poprawnych form:

- Eksport + potem komenda (ustawia na bieżącą sesję shell):

```
export FORMAT_ENABLED=1 WHISPER_VARIANT=large-v3-turbo
uv run python backend.py
```

- Jednolinijkowo, tylko dla tej jednej komendy:

```
FORMAT_ENABLED=1 WHISPER_VARIANT=large-v3-turbo uv run python backend.py
```

Analogicznie dla aplikacji w trayu:

```
FORMAT_ENABLED=1 WHISPER_VARIANT=large-v3-turbo uv run python main.py
```

Wskazówka: jeśli używasz lokalnego LLM, możesz dodać:

```
LLM_ID=./models/bielik-4.5b-mxfp4-mlx FORMAT_ENABLED=1 uv run python backend.py
```

Domyślne ścieżki w repo:
- Ikona: assets/icon.png
- Modele: ./models (Whisper: ./models/whisper-large-v3-turbo)

