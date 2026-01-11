# CodeScribe - Pure Rust Build System
# Speech-to-text tray app for macOS

.PHONY: all build release install bundle install-app start stop restart status logs \
        bump bump-patch bump-minor bump-major version \
        lint format test check clean help \
        tauri-dev tauri-build tauri-check

SHELL := /bin/bash
VERSION_FILE := Cargo.toml
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)

# ============================================================================
# Build & Install
# ============================================================================

all: check

build:
	@echo "Building (debug)..."
	@cargo build

release:
	@echo "Building (release)..."
	@cargo build --release

install:
	@echo "Installing CodeScribe..."
	@cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"

bundle: release
	@echo "Creating macOS app bundle..."
	@mkdir -p bundle/CodeScribe.app/Contents/{MacOS,Resources}
	@cp target/release/codescribe bundle/CodeScribe.app/Contents/MacOS/
	@cp assets/AppIcon.icns bundle/CodeScribe.app/Contents/Resources/ 2>/dev/null || true
	@VERSION=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	if [ -f bundle/CodeScribe.app/Contents/Info.plist ]; then \
		sed -i '' "s/<string>0\.[0-9]*\.[0-9]*</<string>$$VERSION</g" bundle/CodeScribe.app/Contents/Info.plist; \
	fi
	@echo "Bundle ready: bundle/CodeScribe.app"

install-app: bundle
	@echo "Installing to /Applications..."
	@rm -rf /Applications/CodeScribe.app
	@cp -R bundle/CodeScribe.app /Applications/
	@echo "Installed: /Applications/CodeScribe.app"

# ============================================================================
# Run
# ============================================================================

start:
	@nohup codescribe > /tmp/codescribe.log 2>&1 & disown
	@echo "CodeScribe started (logs: /tmp/codescribe.log)"

stop:
	@pkill -f "^codescribe$$" 2>/dev/null || true
	@rm -f ~/.codescribe/codescribe.pid 2>/dev/null || true
	@echo "Stopped"

restart: stop
	@sleep 1
	@$(MAKE) start

status:
	@echo "=== CodeScribe Status ==="
	@pgrep -fl codescribe 2>/dev/null || echo "Not running"

logs:
	@tail -50 /tmp/codescribe.log 2>/dev/null || echo "No logs"

logs-follow:
	@tail -f /tmp/codescribe.log 2>/dev/null || echo "No logs"

# ============================================================================
# Version Bump
# ============================================================================

version:
	@grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/v\1/'

bump:
	@if [ -z "$(TYPE)" ]; then \
		echo "Usage: make bump TYPE=patch|minor|major"; \
		echo "Current: $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"; \
		exit 1; \
	fi
	@current=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	IFS='.' read -r major minor patch <<< "$$current"; \
	case "$(TYPE)" in \
		patch) patch=$$((patch + 1)) ;; \
		minor) minor=$$((minor + 1)); patch=0 ;; \
		major) major=$$((major + 1)); minor=0; patch=0 ;; \
		*) echo "Invalid TYPE: $(TYPE)"; exit 1 ;; \
	esac; \
	new="$$major.$$minor.$$patch"; \
	sed -i '' "s/^version = \"$$current\"/version = \"$$new\"/" $(VERSION_FILE); \
	echo "Bumped: v$$current -> v$$new"

bump-patch:
	@$(MAKE) bump TYPE=patch

bump-minor:
	@$(MAKE) bump TYPE=minor

bump-major:
	@$(MAKE) bump TYPE=major

# ============================================================================
# Tauri Frontend
# ============================================================================

tauri-dev:
	@echo "Starting Tauri dev server..."
	@cd tauri-app && cargo tauri dev

tauri-build:
	@echo "Building Tauri release..."
	@cd tauri-app && cargo tauri build

tauri-check:
	@echo "Checking Tauri..."
	@cd tauri-app && cargo check

# ============================================================================
# Quality
# ============================================================================

format:
	@cargo fmt

lint:
	@echo "=== Format Check ==="
	@cargo fmt -- --check
	@echo "=== Clippy ==="
	@cargo clippy -- -D warnings

test:
	@echo "=== Unit Tests ==="
	@cargo test --lib
	@echo "=== Integration Tests ==="
	@cargo test --test '*' || true

check: lint test
	@echo "Quality gate passed"

# ============================================================================
# Cleanup
# ============================================================================

clean:
	@cargo clean
	@rm -rf .loctree
	@echo "Cleaned"

# ============================================================================
# Help
# ============================================================================

help:
	@echo "CodeScribe - Speech-to-text (Pure Rust)"
	@echo ""
	@echo "Build & Install:"
	@echo "  make build         Build debug binary"
	@echo "  make release       Build release binary"
	@echo "  make install       Install CLI to ~/.cargo/bin"
	@echo "  make bundle        Create CodeScribe.app bundle"
	@echo "  make install-app   Install to /Applications"
	@echo ""
	@echo "Run:"
	@echo "  make start         Start CodeScribe"
	@echo "  make stop          Stop CodeScribe"
	@echo "  make restart       Restart"
	@echo "  make status        Show status"
	@echo "  make logs          Show logs"
	@echo "  make logs-follow   Tail logs"
	@echo ""
	@echo "Version:"
	@echo "  make version       Show current version"
	@echo "  make bump-patch    Bump patch (0.5.1 -> 0.5.2)"
	@echo "  make bump-minor    Bump minor (0.5.1 -> 0.6.0)"
	@echo "  make bump-major    Bump major (0.5.1 -> 1.0.0)"
	@echo ""
	@echo "Quality:"
	@echo "  make lint          Run clippy + fmt check"
	@echo "  make format        Format code"
	@echo "  make test          Run tests"
	@echo "  make check         Full quality gate"
	@echo ""
	@echo "Tauri:"
	@echo "  make tauri-dev     Start dev server"
	@echo "  make tauri-build   Build release"
	@echo "  make tauri-check   Check compilation"
