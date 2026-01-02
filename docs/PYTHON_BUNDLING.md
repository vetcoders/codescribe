# Python Bundling for CodeScribe.app

## Overview

The `bundle_python.sh` script prepares Python source code and dependencies for bundling into the CodeScribe.app macOS application. It copies the necessary Python modules, entry points, and configuration files to the app bundle so the Rust frontend can spawn the Python backend via `uv run python`.

## Architecture

CodeScribe uses a **hybrid architecture**:

- **Frontend**: Rust binary (tray app, hotkey handling, audio capture)
- **Backend**: Python FastAPI server (MLX Whisper transcription, formatting)

The Rust frontend spawns the Python backend as a subprocess:

```bash
uv run python whisper_server.py
```

This means:
1. **`uv` must be installed** on the system (checked at runtime)
2. **Python source code** must be bundled in the app
3. **Dependencies** must be resolvable via `pyproject.toml`

## Script Location

```
packaging/scripts/bundle_python.sh
```

## Usage

The script is automatically called by `build_wrapper_app.sh` during the app build process. Manual invocation:

```bash
packaging/scripts/bundle_python.sh <ROOT_DIR> <APP_DIR>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `ROOT_DIR` | Path to CodeScribe repository root |
| `APP_DIR` | Path to CodeScribe.app bundle being built |

### Example

```bash
./packaging/scripts/bundle_python.sh \
  /Users/you/Codescribe \
  /tmp/CodeScribe.app
```

## What It Does

### 1. Verifies uv Installation

Checks that `uv` is available in PATH. `uv` is required at **runtime** for the Rust frontend to spawn the Python backend.

```bash
$ which uv
/Users/you/.local/bin/uv
$ uv --version
uv 0.8.13
```

### 2. Copies Python Package

Copies the complete `src/codescribe/` package to:

```
$APP_DIR/Contents/Resources/python/codescribe/
```

This includes:
- All `.py` modules
- Subpackages (`formatting/`, `onboarding/`, etc.)
- Package metadata (`__init__.py`, `py.typed`)
- Package assets (`codescribe/assets/`, icons, etc.)

### 3. Copies Entry Point

Copies the whisper_server entry point shim:

```
$APP_DIR/Contents/Resources/python/whisper_server.py
```

This is the script the Rust frontend runs:

```python
# whisper_server.py (root of repo)
from codescribe import whisper_server as _impl
app = _impl.app
# ... runs uvicorn on port 8238
```

### 4. Copies Dependency Manifest

Copies `pyproject.toml` to:

```
$APP_DIR/Contents/Resources/python/pyproject.toml
```

When the backend starts, `uv run python` reads this file to resolve and install dependencies. This allows the app to work even if the system doesn't have all Python packages pre-installed.

### 5. Copies Asset Files

Copies vocabulary and other assets:

```
assets/
├── programming.jsonl    (code vocabulary)
├── veterinary.jsonl     (medical/vet vocabulary)
└── logo.png

codescribe/assets/
├── programming.jsonl    (duplicate for package data)
├── icon.png
└── voice_chat_lab.html  (optional UI assets)
```

### 6. Validates Bundle

Ensures all required files are present:

- `codescribe/__init__.py`
- `codescribe/whisper_server.py`
- `whisper_server.py` (entry point)
- `pyproject.toml`

## Output Structure

```
CodeScribe.app/
└── Contents/
    ├── MacOS/
    │   └── codescribe                    (Rust binary)
    └── Resources/
        ├── python/                       (bundled by this script)
        │   ├── codescribe/               (Python package)
        │   │   ├── __init__.py
        │   │   ├── backend.py
        │   │   ├── whisper_server.py
        │   │   ├── formatting/
        │   │   ├── onboarding/
        │   │   └── assets/
        │   ├── whisper_server.py         (entry point)
        │   ├── pyproject.toml            (dependencies)
        │   └── assets/                   (vocabulary files)
        ├── Models/                       (bundled models)
        │   ├── whisper-small/
        │   └── whisper-large-v3-turbo/
        ├── Repo/                         (full repo copy)
        └── AppIcon.icns
```

## Runtime Flow

When CodeScribe.app launches:

1. **Rust binary** (`codescribe`) starts
2. Checks for existing backend on port 8238
3. If not found, spawns backend:
   ```bash
   cd $APP_DIR/Contents/Resources/python
   uv run python whisper_server.py
   ```
4. `uv` reads `pyproject.toml` and installs dependencies (if needed)
5. FastAPI server starts on `http://127.0.0.1:8238`
6. Rust frontend connects via HTTP, sends audio, gets transcription

## Requirements

### At Build Time

- Source repository structure:
  - `src/codescribe/` (Python package)
  - `whisper_server.py` (entry point shim)
  - `pyproject.toml` (dependency manifest)
  - `assets/` (vocabulary files)

### At Runtime

- `uv` installed on the system
  - Install via: https://docs.astral.sh/uv/getting-started/installation/
  - Or via Homebrew: `brew install uv`
- Python 3.12+ (specified in `pyproject.toml`)
- Internet access to download PyPI packages (first run only, cached after)

## Error Handling

The script provides clear error messages:

```bash
# Missing uv
[✗] uv command not found in PATH
[!] Please install uv: https://docs.astral.sh/uv/getting-started/installation/
[!] CodeScribe requires 'uv run python' to execute the Python backend at runtime.

# Missing source files
[✗] Source package not found: /path/to/server/codescribe
[✗] whisper_server.py entry point not found: /path/to/whisper_server.py
[✗] pyproject.toml not found: /path/to/pyproject.toml

# Validation failures
[✗] Missing required file: codescribe/__init__.py
```

## Integration with Build System

The script is called from `build_wrapper_app.sh`:

```bash
echo "[i] Bundling Python dependencies"
if [[ -f "$ROOT_DIR/packaging/scripts/bundle_python.sh" ]]; then
  "$ROOT_DIR/packaging/scripts/bundle_python.sh" "$ROOT_DIR" "$APP_DIR"
else
  echo "[!] Warning: bundle_python.sh not found. Skipping Python bundling."
fi
```

### Build Order

1. Create app structure (Rust binary, Info.plist)
2. Copy full repo (optimization: exclude heavy dirs)
3. **Bundle Python dependencies** (this script)
4. Bundle Whisper models (MLX)
5. Generate app icon
6. Create DMG

## Troubleshooting

### Python Backend Won't Start

**Symptom**: App launches but no backend on port 8238

**Causes**:
- `uv` not installed
- `pyproject.toml` references unavailable packages
- Python 3.12+ not available

**Solution**:
```bash
# Check uv
which uv
uv --version

# Manually test
cd /path/to/python/bundle
uv run python whisper_server.py --help
```

### Missing Dependencies

**Symptom**: `ModuleNotFoundError: No module named 'mlx_whisper'`

**Cause**: `pyproject.toml` is outdated or references don't match system architecture (Intel vs Apple Silicon)

**Solution**:
```bash
# Update pyproject.toml
cd /repo/root
uv pip freeze > /tmp/deps.txt  # check actual versions

# Rebuild app
packaging/release.sh
```

### Asset Files Missing

**Symptom**: Vocabulary (`.jsonl`) files not found at runtime

**Solution**:
```bash
# Check bundle
ls -la /Applications/CodeScribe.app/Contents/Resources/python/assets/

# Verify source
ls -la assets/*.jsonl
```

## Development Notes

### Adding New Python Modules

1. Add to `src/codescribe/`
2. Rebuild app: `packaging/release.sh`
3. Script automatically includes new modules

### Updating Dependencies

1. Edit `pyproject.toml`
2. Rebuild app: `packaging/release.sh`
3. First app launch will install updated packages via `uv`

### Testing Bundle Locally

```bash
# Simulate bundle structure
mkdir -p /tmp/test_app/Contents/Resources
./packaging/scripts/bundle_python.sh \
  $(pwd) \
  /tmp/test_app

# Verify
ls -la /tmp/test_app/Contents/Resources/python/

# Test backend startup
cd /tmp/test_app/Contents/Resources/python
uv run python whisper_server.py --help
```

## Performance Considerations

- **Initial Launch**: First run downloads and caches PyPI packages (~100-200 MB depending on variant)
- **Subsequent Launches**: Cache hit, backend starts in ~5-10 seconds
- **Bundle Size**: Python package + assets ~50 MB; dependencies cached separately
- **Storage**: Models add 500 MB - 2 GB depending on variants

## Related Files

- `packaging/appwrap/build_wrapper_app.sh` - Main build orchestration
- `packaging/release.sh` - Release build entry point
- `codescribe-rs/src/backend.rs` - Rust backend spawning logic
- `pyproject.toml` - Dependency manifest
- `whisper_server.py` - Entry point shim
