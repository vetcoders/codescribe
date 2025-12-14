# CodeScribe App Bundling Guide

Quick reference for building and bundling CodeScribe.app with Python dependencies.

## Requirements

### System

- macOS 12+
- Xcode command-line tools
- Homebrew

### Dependencies

```bash
# Install uv (required for runtime and bundling)
brew install uv

# Install Rust
rustup update

# Build tools
brew install rsync git git-lfs
```

### Python

- Python 3.12+ (via system or uv)
- `pyproject.toml` must exist in repo root

## Build Process

### One-Command Release Build

```bash
cd /path/to/Codescribe
packaging/release.sh
```

This automatically:
1. Builds Rust frontend (`codescribe`)
2. Creates app bundle structure
3. **Bundles Python dependencies** (via `bundle_python.sh`)
4. Bundles Whisper models
5. Generates DMG
6. Outputs to: `packaging/dmg/CodeScribe-*.dmg`

### Build with Codesigning & Notarization

```bash
export SIGN_IDENTITY="Developer ID Application: Your Name (XXXXXXXXXX)"
export NOTARY_PROFILE=MyProfile  # saved in Keychain

packaging/release.sh
```

### Manual Build Steps

For debugging or custom builds:

```bash
# 1. Build Rust binary
cd codescribe-rs
cargo build --release
cd ..

# 2. Create app structure and bundle everything
packaging/appwrap/build_wrapper_app.sh

# 3. Create DMG (optional)
packaging/dmg/build_dmg.sh

# 4. Sign and notarize (optional)
packaging/scripts/sign_and_notarize.sh \
  --app packaging/dist/CodeScribe.app \
  --identity "Developer ID Application: Your Name (ID)"
```

## Python Bundling Details

### What Gets Bundled

The `bundle_python.sh` script copies:

```
python/
├── codescribe/           (entire package)
│   ├── backend.py
│   ├── whisper_server.py
│   ├── formatting/
│   ├── onboarding/
│   └── assets/
├── whisper_server.py     (entry point)
├── pyproject.toml        (dependencies)
└── assets/               (vocab files)
```

### Dependency Resolution

When the app launches:

1. Rust binary checks for existing backend on port 8238
2. If not found, spawns: `uv run python whisper_server.py`
3. `uv` reads bundled `pyproject.toml`
4. Downloads/installs packages (cached after first run)
5. FastAPI server starts

### Customizing Dependencies

Edit `pyproject.toml` before building:

```toml
[project]
dependencies = [
    "mlx-whisper>=0.4.3",
    "fastapi==0.115.6",
    # ... add/remove as needed
]
```

Then rebuild:

```bash
packaging/release.sh
```

## Testing Before Release

### Local Testing

```bash
# Build without notarization
packaging/release.sh

# Test the app
open packaging/dist/CodeScribe.app

# Check logs
tail -f ~/Library/Logs/CodeScribe.app.log

# Verify backend started
curl http://127.0.0.1:8238/healthz
```

### Verify Bundle Structure

```bash
# Check Python is bundled
ls -la packaging/dist/CodeScribe.app/Contents/Resources/python/

# Check dependencies can resolve
cd packaging/dist/CodeScribe.app/Contents/Resources/python
uv sync  # should work without errors
```

### Test Backend Directly

```bash
# Run backend standalone
cd packaging/dist/CodeScribe.app/Contents/Resources/python
PORT=8238 uv run python whisper_server.py

# In another terminal
curl http://127.0.0.1:8238/healthz
```

## Troubleshooting

### Build Fails: uv not found

```bash
brew install uv
which uv  # confirm it's in PATH
```

### Build Fails: bundle_python.sh missing

```bash
# File should exist
ls -la packaging/scripts/bundle_python.sh

# If missing, file was deleted; restore from git
git checkout packaging/scripts/bundle_python.sh
```

### Python Backend Won't Start

**Check logs**:
```bash
tail ~/Library/Logs/CodeScribe.app.log
```

**Common issues**:
- `uv` not in PATH → add `~/.local/bin:/opt/homebrew/bin` to PATH
- Python 3.12+ not available → `uv` handles this
- Port 8238 in use → kill process: `lsof -i :8238`

**Test manually**:
```bash
cd "$(dirname "$(which uv)")/../share/python"
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
uv run python /path/to/whisper_server.py
```

### Assets Not Found

**Check bundle**:
```bash
ls -la packaging/dist/CodeScribe.app/Contents/Resources/python/assets/
# Should have: programming.jsonl, veterinary.jsonl, etc.
```

**Rebuild if missing**:
```bash
packaging/release.sh
```

### Notarization Fails

See `packaging/scripts/notary_quick.sh` for details:

```bash
# Verify signing
codesign -dv packaging/dist/CodeScribe.app

# Notarize manually
xcrun altool --notarize-app \
  -f packaging/dmg/CodeScribe-*.dmg \
  -t osx \
  -u YOUR_APPLE_ID \
  -p APP_PASSWORD \
  --team-id TEAM_ID
```

## CI/CD Integration

For GitHub Actions or other CI:

```bash
#!/bin/bash
set -euo pipefail

# Install dependencies
brew install uv rustup rsync

# Build
cd $GITHUB_WORKSPACE/Codescribe
packaging/release.sh

# Upload artifact
mkdir -p artifacts
cp packaging/dmg/CodeScribe-*.dmg artifacts/
```

## File Locations

| File | Purpose |
|------|---------|
| `packaging/scripts/bundle_python.sh` | Main Python bundling script |
| `packaging/appwrap/build_wrapper_app.sh` | App builder (calls bundle_python.sh) |
| `packaging/release.sh` | Release build orchestrator |
| `packaging/PYTHON_BUNDLING.md` | Detailed bundling docs |
| `pyproject.toml` | Python dependencies |
| `codescribe-rs/Cargo.toml` | Rust dependencies |

## Key Concepts

### Option A: `uv run` (Current)

- **Pro**: Lightweight, single source of truth (pyproject.toml), works offline after first run
- **Con**: Requires `uv` at runtime
- **Status**: Implemented

### Option B: Embedded venv (Alternative)

- **Pro**: Completely self-contained
- **Con**: Large bundle size, updates harder
- **Status**: Not implemented

**Current approach (Option A) is preferred** because:
- `uv` is lightweight and fast
- Standard packaging approach
- Smaller bundle size
- Easier to update dependencies

## Architecture

```
Finder launches
  ↓
CodeScribe.app (macOS app bundle)
  ├── MacOS/codescribe (Rust binary)
  └── Resources/
      ├── python/ (bundled by bundle_python.sh)
      │   ├── codescribe/ (Python package)
      │   ├── pyproject.toml (dependencies)
      │   ├── whisper_server.py (entry point)
      │   └── assets/ (vocab files)
      ├── Models/ (Whisper models)
      └── Repo/ (full repo copy)

Rust binary executes:
  ↓
uv run python whisper_server.py
  ├── Reads: Resources/python/pyproject.toml
  ├── Installs: dependencies (cached)
  └── Starts: FastAPI on port 8238

User records audio via hotkey
  ↓
Rust sends audio to Python backend
  ↓
Python transcribes with MLX Whisper
  ↓
Result pasted to active application
```

## Support

For issues or questions:
- Check logs: `~/Library/Logs/CodeScribe.app.log`
- Review docs: `packaging/PYTHON_BUNDLING.md`
- Test manually: `cd [python dir]; uv run python whisper_server.py`
