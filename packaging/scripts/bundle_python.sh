#!/bin/zsh
#
# bundle_python.sh — Bundle Python dependencies for CodeScribe.app
#
# This script copies the Python source code and configuration needed by the Rust frontend
# to run the Python backend via `uv run python`.
#
# Usage:
#   bundle_python.sh <ROOT_DIR> <APP_DIR>
#
# Arguments:
#   ROOT_DIR  — Path to CodeScribe repository root
#   APP_DIR   — Path to CodeScribe.app bundle
#
# The script will:
#   1. Verify uv is installed (required at runtime)
#   2. Copy server/codescribe/ Python package
#   3. Copy whisper_server.py and whisper_server module
#   4. Copy pyproject.toml for uv to resolve dependencies
#   5. Copy assets (*.jsonl vocabulary files, etc.)
#   6. Copy scripts (get_models.py for model download)
#   7. Validate the bundle structure
#
# Output directory structure:
#   $APP_DIR/Contents/Resources/python/
#   ├── codescribe/          (Python package)
#   ├── whisper_server.py    (entry point for uvicorn)
#   ├── pyproject.toml       (dependency manifest for uv)
#   ├── assets/              (vocabulary files, etc.)
#   └── scripts/             (get_models.py, etc.)

set -euo pipefail

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

print_info() {
  echo -e "${BLUE}[i]${NC} $1"
}

print_success() {
  echo -e "${GREEN}[✓]${NC} $1"
}

print_warning() {
  echo -e "${YELLOW}[!]${NC} $1"
}

print_error() {
  echo -e "${RED}[✗]${NC} $1"
}

# Verify arguments
if [[ $# -ne 2 ]]; then
  print_error "Usage: bundle_python.sh <ROOT_DIR> <APP_DIR>"
  exit 1
fi

ROOT_DIR="$1"
APP_DIR="$2"

# Verify paths exist
if [[ ! -d "$ROOT_DIR" ]]; then
  print_error "ROOT_DIR does not exist: $ROOT_DIR"
  exit 1
fi

if [[ ! -d "$APP_DIR/Contents/Resources" ]]; then
  print_error "APP_DIR structure incomplete: $APP_DIR/Contents/Resources"
  exit 1
fi

PYTHON_DST="$APP_DIR/Contents/Resources/python"
mkdir -p "$PYTHON_DST"

print_info "Bundling Python dependencies"
print_info "  Root: $ROOT_DIR"
print_info "  Dest: $PYTHON_DST"

# Step 1: Verify uv is installed
print_info "Verifying uv installation..."
if ! command -v uv >/dev/null 2>&1; then
  print_error "uv command not found in PATH"
  print_warning "Please install uv: https://docs.astral.sh/uv/getting-started/installation/"
  print_warning "CodeScribe requires 'uv run python' to execute the Python backend at runtime."
  exit 1
fi
UV_VERSION=$(uv --version 2>/dev/null || true)
print_success "uv found: $UV_VERSION"

# Step 2: Copy Python source package
print_info "Copying codescribe Python package..."
SRC_PACKAGE="$ROOT_DIR/src/codescribe"
if [[ ! -d "$SRC_PACKAGE" ]]; then
  print_error "Source package not found: $SRC_PACKAGE"
  exit 1
fi
cp -R "$SRC_PACKAGE" "$PYTHON_DST/codescribe"
print_success "Copied Python package ($(find "$PYTHON_DST/codescribe" -type f -name '*.py' | wc -l) Python files)"

# Step 3: Copy whisper_server entry point
print_info "Copying whisper_server.py entry point..."
WHISPER_ENTRY="$ROOT_DIR/whisper_server.py"
if [[ ! -f "$WHISPER_ENTRY" ]]; then
  print_error "whisper_server.py entry point not found: $WHISPER_ENTRY"
  exit 1
fi
cp "$WHISPER_ENTRY" "$PYTHON_DST/whisper_server.py"
print_success "Copied whisper_server.py"

# Step 4: Copy pyproject.toml
print_info "Copying pyproject.toml for dependency resolution..."
PYPROJECT="$ROOT_DIR/pyproject.toml"
if [[ ! -f "$PYPROJECT" ]]; then
  print_error "pyproject.toml not found: $PYPROJECT"
  exit 1
fi
cp "$PYPROJECT" "$PYTHON_DST/pyproject.toml"
print_success "Copied pyproject.toml"

# Step 5: Copy assets (vocabulary files, etc.)
print_info "Copying assets (vocabulary, etc.)..."
ASSETS_SRC="$ROOT_DIR/assets"
if [[ -d "$ASSETS_SRC" ]]; then
  ASSETS_DST="$PYTHON_DST/assets"
  mkdir -p "$ASSETS_DST"

  # Copy vocabulary JSONL files
  if find "$ASSETS_SRC" -maxdepth 1 -name "*.jsonl" -type f >/dev/null 2>&1; then
    cp "$ASSETS_SRC"/*.jsonl "$ASSETS_DST/" 2>/dev/null || true
    FILE_COUNT=$(find "$ASSETS_DST" -name "*.jsonl" -type f | wc -l)
    print_success "Copied $FILE_COUNT vocabulary files"
  fi

  # Copy other static assets if present
  if find "$ASSETS_SRC" -maxdepth 1 -type f -not -name "*.jsonl" >/dev/null 2>&1; then
    for f in "$ASSETS_SRC"/*; do
      [[ -f "$f" && ! "$f" =~ \.DS_Store$ ]] && cp "$f" "$ASSETS_DST/" 2>/dev/null || true
    done
  fi
else
  print_warning "Assets directory not found: $ASSETS_SRC (optional)"
fi

# Step 6: Copy scripts (get_models.py, etc.)
print_info "Copying scripts (get_models.py, etc.)..."
SCRIPTS_SRC="$ROOT_DIR/scripts"
if [[ -d "$SCRIPTS_SRC" ]]; then
  SCRIPTS_DST="$PYTHON_DST/scripts"
  mkdir -p "$SCRIPTS_DST"
  cp "$SCRIPTS_SRC"/*.py "$SCRIPTS_DST/" 2>/dev/null || true
  SCRIPT_COUNT=$(find "$SCRIPTS_DST" -name "*.py" -type f | wc -l)
  print_success "Copied $SCRIPT_COUNT script files"
else
  print_warning "Scripts directory not found: $SCRIPTS_SRC (model download may not work)"
fi

# Step 7: Copy package data from codescribe/assets if different from assets/
if [[ -d "$SRC_PACKAGE/assets" && "$SRC_PACKAGE/assets" != "$ASSETS_SRC" ]]; then
  print_info "Copying codescribe package assets..."
  PACKAGE_ASSETS="$PYTHON_DST/codescribe/assets"
  if [[ ! -d "$PACKAGE_ASSETS" ]]; then
    mkdir -p "$PACKAGE_ASSETS"
    if [[ -d "$SRC_PACKAGE/assets" ]]; then
      cp -R "$SRC_PACKAGE/assets"/* "$PACKAGE_ASSETS/" 2>/dev/null || true
      print_success "Copied package assets"
    fi
  fi
fi

# Step 8: Validate bundle structure
print_info "Validating bundle structure..."
VALIDATION_OK=true

# Check required files
for required_file in \
  "codescribe/__init__.py" \
  "codescribe/whisper_server.py" \
  "whisper_server.py" \
  "pyproject.toml"; do
  if [[ ! -f "$PYTHON_DST/$required_file" ]]; then
    print_error "Missing required file: $required_file"
    VALIDATION_OK=false
  fi
done

# Check codescribe package structure
if [[ ! -d "$PYTHON_DST/codescribe" ]]; then
  print_error "Missing codescribe package directory"
  VALIDATION_OK=false
fi

if $VALIDATION_OK; then
  print_success "Bundle validation passed"

  # Summary
  PACKAGE_FILES=$(find "$PYTHON_DST/codescribe" -name "*.py" -type f | wc -l)
  ASSET_FILES=$(find "$PYTHON_DST/assets" -type f 2>/dev/null | wc -l || echo 0)

  echo ""
  print_info "Python bundle summary:"
  echo "  Location:       $PYTHON_DST"
  echo "  Package files:  $PACKAGE_FILES Python modules"
  echo "  Asset files:    $ASSET_FILES files"
  echo "  Entry point:    whisper_server.py"
  echo ""
  echo "To run the backend at runtime:"
  echo "  cd $PYTHON_DST"
  echo "  uv run python whisper_server.py"
  echo ""
else
  print_error "Bundle validation failed"
  exit 1
fi

print_success "Python bundle complete"
exit 0
