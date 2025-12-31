# CodeScribe - Unified Build System
# Speech-to-text tray app for macOS (Rust) + Python backend

.PHONY: all build release install bundle install-app start stop restart status logs logs-app logs-backend logs-follow config \
        bump bump-patch bump-minor bump-major version \
        lint format test security check clean help \
        backend-start backend-stop

SHELL := /bin/bash
VERSION_FILE := codescribe-rs/Cargo.toml
ENV_FILE := ~/.codescribe/.env
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)

# ============================================================================
# Build & Install
# ============================================================================

all: check

build:
	@echo "Building Rust binary (debug)..."
	@cd codescribe-rs && cargo build

release:
	@echo "Building Rust binary (release)..."
	@cd codescribe-rs && cargo build --release

install:
	@echo "Installing CodeScribe..."
	@cd codescribe-rs && cargo install --path . --force
	@echo "✅ Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"

bundle: install
	@echo "Creating macOS app bundle..."
	@mkdir -p codescribe-rs/bundle/CodeScribe.app/Contents/{MacOS,Resources}
	@cp codescribe-rs/bundle/CodeScribe.app/Contents/Info.plist codescribe-rs/bundle/CodeScribe.app/Contents/ 2>/dev/null || true
	@cp codescribe-rs/assets/AppIcon.icns codescribe-rs/bundle/CodeScribe.app/Contents/Resources/
	@VERSION=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	sed -i '' "s/<string>0\.[0-9]*\.[0-9]*</<string>$$VERSION</g" codescribe-rs/bundle/CodeScribe.app/Contents/Info.plist
	@echo "✅ Bundle ready: codescribe-rs/bundle/CodeScribe.app"

install-app: bundle
	@echo "Installing to /Applications..."
	@rm -rf /Applications/CodeScribe.app
	@cp -R codescribe-rs/bundle/CodeScribe.app /Applications/
	@echo "✅ Installed: /Applications/CodeScribe.app"
	@echo "   You can now launch CodeScribe from Spotlight or Launchpad"

# ============================================================================
# Run
# ============================================================================

start: backend-start
	@nohup codescribe > /tmp/codescribe.log 2>&1 & disown
	@echo "✅ CodeScribe started (logs: /tmp/codescribe.log)"

stop:
	@pkill -f "^codescribe$$" 2>/dev/null || true
	@pkill -f "python.*codescribe.backend" 2>/dev/null || true
	@rm -f ~/.codescribe/codescribe.pid 2>/dev/null || true
	@echo "✅ Stopped"

restart: stop
	@sleep 1
	@$(MAKE) start

status:
	@echo "=== CodeScribe Status ==="
	@pgrep -fl codescribe 2>/dev/null || echo "Not running"
	@echo ""
	@echo "=== Backend Status ==="
	@curl -s http://127.0.0.1:8237/healthz 2>/dev/null || echo "Backend not responding"

logs:
	@echo "=== Backend Logs (last 30) ==="
	@cat /tmp/codescribe-backend.log 2>/dev/null | tail -30 || echo "No backend logs"
	@echo ""
	@echo "=== App Logs (last 30) ==="
	@cat /tmp/codescribe.log 2>/dev/null | tail -30 || echo "No app logs"

logs-backend:
	@tail -100 /tmp/codescribe-backend.log 2>/dev/null || echo "No backend logs"

logs-app:
	@tail -100 /tmp/codescribe.log 2>/dev/null || echo "No app logs"

logs-follow:
	@tail -f /tmp/codescribe.log /tmp/codescribe-backend.log 2>/dev/null || echo "No logs"

# ============================================================================
# Configuration
# ============================================================================

config:
	@mkdir -p ~/.codescribe
	@if [ ! -f "$(ENV_FILE)" ]; then \
		cp .env "$(ENV_FILE)" 2>/dev/null || touch "$(ENV_FILE)"; \
		echo "Created $(ENV_FILE)"; \
	fi
	@$(EDITOR) "$(ENV_FILE)"

config-show:
	@echo "=== $(ENV_FILE) ==="
	@cat "$(ENV_FILE)" 2>/dev/null || echo "No config found"

config-copy:
	@mkdir -p ~/.codescribe
	@cp .env "$(ENV_FILE)"
	@echo "✅ Copied .env → $(ENV_FILE)"

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
	echo "✅ Bumped: v$$current → v$$new"

bump-patch:
	@$(MAKE) bump TYPE=patch

bump-minor:
	@$(MAKE) bump TYPE=minor

bump-major:
	@$(MAKE) bump TYPE=major

# ============================================================================
# Backend (Python)
# ============================================================================

backend-start:
	@if curl -s http://127.0.0.1:8237/healthz >/dev/null 2>&1; then \
		echo "Backend already running"; \
	else \
		echo "Starting Python backend..."; \
		nohup uv run python -m codescribe.backend > /tmp/codescribe-backend.log 2>&1 & \
		for i in 1 2 3 4 5 6 7 8 9 10; do \
			sleep 1; \
			if curl -s http://127.0.0.1:8237/healthz >/dev/null 2>&1; then \
				echo "✅ Backend started on :8237 ($$i sec)"; \
				exit 0; \
			fi; \
			printf "."; \
		done; \
		echo ""; \
		echo "⏳ Backend still starting (Whisper loading). Continuing..."; \
	fi

backend-stop:
	@pkill -f "python.*codescribe.backend" 2>/dev/null && echo "✅ Backend stopped" || echo "Backend not running"

backend-logs:
	@cat /tmp/codescribe-backend.log 2>/dev/null | tail -100

# ============================================================================
# Linting & Testing
# ============================================================================

format:
	@echo "Formatting Python..."
	@uv run ruff format .
	@echo "Formatting Rust..."
	@cd codescribe-rs && cargo fmt

lint:
	@echo "=== Python Lint ==="
	@uv run ruff check .
	@uv run mypy src/codescribe
	@echo "=== Rust Lint ==="
	@cd codescribe-rs && cargo fmt -- --check
	@cd codescribe-rs && cargo clippy -- -D warnings

security:
	@echo "Running Bandit..."
	@uv run bandit -ll -c pyproject.toml -r src/codescribe

test:
	@echo "=== Python Tests ==="
	@uv run pytest
	@echo "=== Rust Tests ==="
	@cd codescribe-rs && cargo test

check: lint security test

# ============================================================================
# Cleanup
# ============================================================================

clean:
	@cd codescribe-rs && cargo clean
	@rm -rf .pytest_cache .ruff_cache .mypy_cache
	@find . -name "__pycache__" -type d -exec rm -rf {} + 2>/dev/null || true
	@echo "✅ Cleaned"

# ============================================================================
# Help
# ============================================================================

help:
	@echo "CodeScribe - Speech-to-text tray app"
	@echo ""
	@echo "Build & Install:"
	@echo "  make build         Build debug binary"
	@echo "  make release       Build release binary"
	@echo "  make install       Install CLI to ~/.cargo/bin"
	@echo "  make bundle        Create CodeScribe.app bundle"
	@echo "  make install-app   Install to /Applications"
	@echo ""
	@echo "Run:"
	@echo "  make start         Start CodeScribe + backend"
	@echo "  make stop          Stop all processes"
	@echo "  make restart       Restart everything"
	@echo "  make status        Show status"
	@echo "  make logs          Show all logs (app + backend)"
	@echo "  make logs-app      Show app logs only"
	@echo "  make logs-backend  Show backend logs only"
	@echo "  make logs-follow   Tail all logs"
	@echo ""
	@echo "Configuration:"
	@echo "  make config        Edit ~/.codescribe/.env"
	@echo "  make config-show   Show current config"
	@echo "  make config-copy   Copy .env to ~/.codescribe/"
	@echo ""
	@echo "Version:"
	@echo "  make version       Show current version"
	@echo "  make bump-patch    0.5.1 → 0.5.2"
	@echo "  make bump-minor    0.5.1 → 0.6.0"
	@echo "  make bump-major    0.5.1 → 1.0.0"
	@echo ""
	@echo "Quality:"
	@echo "  make lint          Run all linters"
	@echo "  make format        Format all code"
	@echo "  make test          Run all tests"
	@echo "  make check         Full CI pipeline"
	@echo ""
	@echo "Backend:"
	@echo "  make backend-start Start Python backend only"
	@echo "  make backend-stop  Stop Python backend only"
