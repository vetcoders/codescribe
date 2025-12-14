# Bundled App Mode Support

This document describes how CodeScribe supports running from a macOS .app bundle.

## Overview

CodeScribe can run in two modes:

1. **Development Mode**: Running from source repository (`cargo run` or standalone binary)
2. **Bundled Mode**: Running from CodeScribe.app bundle (distributed via DMG)

## .app Bundle Structure

When bundled, the app has this structure:

```
CodeScribe.app/
├── Contents/
│   ├── MacOS/
│   │   └── codescribe              # Rust binary
│   ├── Resources/
│   │   └── python/
│   │       ├── codescribe/         # Python package
│   │       ├── whisper_server.py   # Entry point
│   │       ├── pyproject.toml      # Dependencies
│   │       └── assets/             # Vocabulary files (*.jsonl)
│   └── Info.plist
```

## Path Resolution

### Python Backend (whisper_server.py)

The Rust frontend looks for `whisper_server.py` in this order:

1. `$CODESCRIBE_PYTHON_DIR/whisper_server.py` (if env var set)
2. `../Resources/python/whisper_server.py` (bundled mode)
3. `$CARGO_MANIFEST_DIR/../whisper_server.py` (development mode)
4. `./whisper_server.py` (current directory)
5. `../whisper_server.py` (parent directory)

**Implementation**: See `find_whisper_server()` in `codescribe-rs/src/backend.rs`

### Vocabulary Assets (*.jsonl)

Python looks for vocabulary assets in this order:

1. `$CODESCRIBE_ASSETS_DIR/*.jsonl` (if env var set)
2. `<python_module_dir>/../assets/*.jsonl` (bundled mode)
3. `repo_root()/src/codescribe/assets/*.jsonl` (development mode)
4. `repo_root()/assets/*.jsonl` (development mode)

**Implementation**: See `_asset_roots()` in `src/codescribe/formatting/vocabulary.py`

### Whisper Models

Models are downloaded to:

- **Bundled mode**: `$HOME/.CodeScribe/models/whisper-{variant}/`
- **Development mode**: `repo_root/models/whisper-{variant}/`

Models are downloaded on first run if not present.

**Implementation**: See `ensure_models_exist()` in `codescribe-rs/src/backend.rs`

## Logging

### Development Mode

Logs go to stdout/stderr with ANSI colors (if TTY).

### Bundled Mode (Launched from Finder)

When launched from Finder (no TTY), logs are written to:

- File: `$HOME/Library/Logs/CodeScribe.log`
- System logs: Use `log stream --predicate 'process == "codescribe"'` to view

**Implementation**: See `main()` in `codescribe-rs/src/main.rs`

## Environment Variables

You can override default paths with these environment variables:

| Variable | Purpose | Example |
|----------|---------|---------|
| `CODESCRIBE_PYTHON_DIR` | Override Python files location | `/path/to/custom/python` |
| `CODESCRIBE_ASSETS_DIR` | Override vocabulary assets location | `/path/to/custom/assets` |
| `CODESCRIBE_DATA_DIR` | Override user data directory | `$HOME/.CodeScribe` |
| `WHISPER_VARIANT` | Choose Whisper model variant | `small`, `base`, `medium` |

## Working Directory for uv

When spawning the Python backend, the Rust code sets the working directory to:

- If `CODESCRIBE_PYTHON_DIR` is set: Use that directory
- Otherwise: Use the directory containing `whisper_server.py`

This ensures `uv run python` can find `pyproject.toml` for dependency resolution.

**Implementation**: See `BackendServer::start()` in `codescribe-rs/src/backend.rs`

## Testing Bundled Mode

To test bundled mode detection without building the full .app:

```bash
# Set environment to simulate bundled mode
export CODESCRIBE_PYTHON_DIR="/path/to/CodeScribe.app/Contents/Resources/python"
export CODESCRIBE_ASSETS_DIR="/path/to/CodeScribe.app/Contents/Resources/python/assets"

# Run the app
./codescribe-rs/target/debug/codescribe
```

## Building the Bundle

The bundling process is handled by:

1. `packaging/scripts/bundle_python.sh` - Copies Python files and assets to .app bundle
2. `packaging/release.sh` - Builds and signs the complete app

The bundle script ensures all necessary files are copied:
- Python source code (`src/codescribe/`)
- Entry point (`whisper_server.py`)
- Dependencies manifest (`pyproject.toml`)
- Vocabulary assets (`assets/*.jsonl`)

## Troubleshooting

### Backend fails to start in bundled mode

Check these common issues:

1. **Missing Python files**: Verify `Contents/Resources/python/` exists
2. **Missing uv**: Install with `curl -LsSf https://astral.sh/uv/install.sh | sh`
3. **Check logs**: `tail -f ~/Library/Logs/CodeScribe.log`

### Vocabulary not loading

1. **Verify assets exist**: Check `Contents/Resources/python/assets/*.jsonl`
2. **Override with env var**: `export CODESCRIBE_ASSETS_DIR=/path/to/assets`
3. **Check Python logs**: Look for vocabulary loading errors in backend logs

### Models not downloading

1. **Check data directory**: Models should be in `~/.CodeScribe/models/`
2. **Internet connection**: First run requires downloading ~500MB
3. **Disk space**: Ensure sufficient space for models

## Implementation Details

### Detection Logic

The code detects bundled mode by checking if:

```rust
exe_path
    .to_str()
    .map(|s| s.contains("/CodeScribe.app/"))
    .unwrap_or(false)
```

Or by checking if the bundled Python path exists:

```rust
exe_dir.join("../Resources/python/whisper_server.py").exists()
```

### Backwards Compatibility

All changes maintain backwards compatibility with development mode:

- If environment variables are not set, fall back to development paths
- If bundled paths don't exist, try development paths
- Logging gracefully degrades to stdout if file logging fails

## See Also

- `packaging/scripts/bundle_python.sh` - Bundle creation script
- `codescribe-rs/src/backend.rs` - Backend spawning logic
- `src/codescribe/formatting/vocabulary.py` - Asset loading logic
- `src/codescribe/path_utils.py` - Path resolution utilities
