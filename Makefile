# CodeScribe - Pure Rust Build System
# Speech-to-text tray app for macOS

.PHONY: all build release install install-no-embed config bundle install-app \
        start stop restart status logs logs-follow \
        bump bump-patch bump-minor bump-major version \
        lint format test test-quick test-e2e test-e2e-real test-sse test-formatting test-all \
        demo demo-raw demo-assistive check fix clean help \
        dmg dmg-signed release-full notarize download-model download-e5 download-embedder ensure-models \
        hooks

SHELL := /bin/bash
VERSION_FILE := Cargo.toml
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)
ENV_LOAD := set -a; [ -f $$HOME/.codescribe/.env ] && source $$HOME/.codescribe/.env; set +a
# macOS: use a stable codesign identity to avoid TCC (Accessibility/Input Monitoring) resets after rebuilds.
# Example:
#   CODESCRIBE_CODESIGN_IDENTITY="Apple Development: Your Name (TEAMID)" make install-app
CODESCRIBE_CODESIGN_IDENTITY ?= -
CODESCRIBE_APP_NAME ?= CodeScribe
CODESCRIBE_DISPLAY_NAME ?= CodeScribe
CODESCRIBE_BUNDLE_ID ?= com.codescribe.app
CODESCRIBE_MIN_MACOS ?=
CODESCRIBE_LSUIELEMENT ?= true
CODESCRIBE_ENTITLEMENTS ?= scripts/entitlements.plist

# Test defaults (reference/cloud unless forced local)
TEST_USE_LOCAL_LLM ?= 0
LOCAL_LLM_ENDPOINT ?= http://localhost:11434/v1/responses
LOCAL_LLM_MODEL ?= gpt-oss:120b-cloud
LOCAL_LLM_API_KEY ?= local

define APPLY_TEST_LLM
if [[ "$(TEST_USE_LOCAL_LLM)" == "1" ]]; then \
  export LLM_ENDPOINT="$(LOCAL_LLM_ENDPOINT)"; \
  export LLM_MODEL="$(LOCAL_LLM_MODEL)"; \
  export LLM_API_KEY="$(LOCAL_LLM_API_KEY)"; \
  export LLM_FORMATTING_ENDPOINT="$(LOCAL_LLM_ENDPOINT)"; \
  export LLM_FORMATTING_MODEL="$(LOCAL_LLM_MODEL)"; \
  export LLM_FORMATTING_API_KEY="$(LOCAL_LLM_API_KEY)"; \
  export LLM_ASSISTIVE_ENDPOINT="$(LOCAL_LLM_ENDPOINT)"; \
  export LLM_ASSISTIVE_MODEL="$(LOCAL_LLM_MODEL)"; \
  export LLM_ASSISTIVE_API_KEY="$(LOCAL_LLM_API_KEY)"; \
  export LLM_USE_STREAMING=1; \
fi
endef

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
	@echo "Installing CodeScribe (with embedded model)..."
	@./scripts/ensure-models.sh
	@cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"

install-no-embed:
	@echo "Installing CodeScribe (no embedded model)..."
	@./scripts/ensure-models.sh
	@CODESCRIBE_NO_EMBED=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"
	@echo "Note: Set CODESCRIBE_MODEL_PATH at runtime"

config:
	@mkdir -p ~/.codescribe
	@if [ ! -f ~/.codescribe/.env ]; then \
		cp .env.example ~/.codescribe/.env 2>/dev/null || echo "# CodeScribe Config" > ~/.codescribe/.env; \
		echo "Created ~/.codescribe/.env"; \
	fi
	@$(EDITOR) ~/.codescribe/.env

bundle: ensure-models release
	@echo "Creating macOS app bundle..."
	@mkdir -p bundle/$(CODESCRIBE_APP_NAME).app/Contents/{MacOS,Resources}
	@cp target/release/codescribe bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/
	@cp target/release/codescribe-loop bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/ 2>/dev/null || true
	@cp target/release/codescribe-quality bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/ 2>/dev/null || true
	@cp assets/AppIcon.icns bundle/$(CODESCRIBE_APP_NAME).app/Contents/Resources/ 2>/dev/null || true
	@VERSION=$$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0">' \
		'<dict>' \
		"  <key>CFBundleName</key><string>$(CODESCRIBE_APP_NAME)</string>" \
		"  <key>CFBundleDisplayName</key><string>$(CODESCRIBE_DISPLAY_NAME)</string>" \
		"  <key>CFBundleIdentifier</key><string>$(CODESCRIBE_BUNDLE_ID)</string>" \
		"  <key>CFBundleVersion</key><string>$$VERSION</string>" \
		"  <key>CFBundleShortVersionString</key><string>$$VERSION</string>" \
		'  <key>CFBundlePackageType</key><string>APPL</string>' \
		'  <key>CFBundleExecutable</key><string>codescribe</string>' \
		'  <key>CFBundleIconFile</key><string>AppIcon</string>' \
		"  <key>LSUIElement</key><$(CODESCRIBE_LSUIELEMENT)/>" \
		'  <key>NSMicrophoneUsageDescription</key><string>Needed to transcribe speech.</string>' \
		'  <key>NSAccessibilityUsageDescription</key><string>Needed to monitor hotkeys and paste results.</string>' \
		'  <key>NSInputMonitoringUsageDescription</key><string>Needed to detect global hotkeys.</string>' \
		'  <key>NSScreenCaptureUsageDescription</key><string>Capture screen context for AI-assisted features.</string>' \
		'</dict>' \
		'</plist>' \
		> bundle/$(CODESCRIBE_APP_NAME).app/Contents/Info.plist; \
	if [ -n "$(CODESCRIBE_MIN_MACOS)" ]; then \
		/usr/libexec/PlistBuddy -c "Add :LSMinimumSystemVersion string $(CODESCRIBE_MIN_MACOS)" bundle/$(CODESCRIBE_APP_NAME).app/Contents/Info.plist >/dev/null 2>&1 || true; \
	fi
	@echo "Bundle ready: bundle/$(CODESCRIBE_APP_NAME).app"

install-app: bundle
	@echo "Installing to /Applications..."
	@mkdir -p /Applications
	@rsync -a --delete bundle/$(CODESCRIBE_APP_NAME).app/ /Applications/$(CODESCRIBE_APP_NAME).app/
	@if [ "$(CODESCRIBE_CODESIGN_IDENTITY)" = "-" ]; then \
		echo "Codesigning ad-hoc (no signing identity found)."; \
		echo "NOTE: macOS Accessibility/Input Monitoring may need re-grant after reinstall."; \
		echo "TIP: create a local codesign cert (e.g. 'CodeScribe Dev') and set CODESCRIBE_CODESIGN_IDENTITY to keep permissions stable."; \
		codesign --force --deep --sign - --identifier $(CODESCRIBE_BUNDLE_ID) /Applications/$(CODESCRIBE_APP_NAME).app; \
	else \
		echo "Codesigning with identity: $(CODESCRIBE_CODESIGN_IDENTITY)"; \
		codesign --force --deep --options runtime --entitlements "$(CODESCRIBE_ENTITLEMENTS)" --sign "$(CODESCRIBE_CODESIGN_IDENTITY)" --identifier $(CODESCRIBE_BUNDLE_ID) /Applications/$(CODESCRIBE_APP_NAME).app; \
	fi
	@echo "Codesign summary:"
	@codesign --display --verbose=2 /Applications/$(CODESCRIBE_APP_NAME).app 2>&1 | sed -n '1,12p' || true
	@echo "Installed: /Applications/$(CODESCRIBE_APP_NAME).app"

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
# Quality
# ============================================================================

format:
	@cargo fmt

lint:
	@echo "=== Format Check ==="
	@cargo fmt -- --check
	@echo "=== Clippy ==="
	@cargo clippy --workspace -- -D warnings

TEST_LOG := /tmp/codescribe-tests.log

define TEST_SETUP
LOG=$(TEST_LOG); \
export CODESCRIBE_DISABLE_KEYCHAIN=1; \
echo "" >> "$$LOG"; \
echo "╔══════════════════════════════════════════════════════════╗" | tee -a "$$LOG"; \
echo "║  CodeScribe Test Suite — $$(date '+%Y-%m-%d %H:%M:%S')           ║" | tee -a "$$LOG"; \
echo "╚══════════════════════════════════════════════════════════╝" | tee -a "$$LOG"; \
open -a Console "$$LOG"
endef

test:
	@$(TEST_SETUP); \
	echo "=== Tests (workspace) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== Tests (ignored / real API) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== Full Pipeline (STT) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); CODESCRIBE_E2E_STT=1 \
	cargo test --test e2e_full_pipeline -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-quick:
	@$(TEST_SETUP); \
	echo "=== Tests (quick, no real API) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-e2e:
	@$(TEST_SETUP); \
	echo "=== E2E Tests (mock) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e --release -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-e2e-real:
	@$(TEST_SETUP); \
	echo "=== E2E Tests (real API) ===" | tee -a "$$LOG"; \
	echo "Requires: LLM_API_KEY, LLM_ASSISTIVE_API_KEY" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e --release -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-sse:
	@$(TEST_SETUP); \
	echo "=== SSE Streaming Tests ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e_sse --release -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== Responses Live Chain/Resume Tests ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); CODESCRIBE_E2E_RESPONSES=1 \
	cargo test --test e2e_retry_responses -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-formatting:
	@$(TEST_SETUP); \
	echo "=== AI Formatting Tests ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test formatting --release -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-all:
	@$(TEST_SETUP); \
	echo "=== Full Test Suite ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== Ignored / Real API ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== Full Pipeline (STT) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); CODESCRIBE_E2E_STT=1 \
	cargo test --test e2e_full_pipeline -- --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "=== SSE Streaming ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e_sse --release -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

demo:
	@echo "=== Full Pipeline Demo ==="
	@cargo run --release --example demo_full_pipeline -- $(AUDIO)

demo-raw:
	@echo "=== Raw Transcription Demo ==="
	@cargo run --release --example demo_full_pipeline -- --raw $(AUDIO)

demo-assistive:
	@echo "=== Assistive Mode Demo ==="
	@cargo run --release --example demo_full_pipeline -- --assistive $(AUDIO)

check:
	@echo "=== Format Check (Rust) ==="
	@cargo fmt --all -- --check
	@echo "=== Format Check (non-Rust) ==="
	@npx --yes prettier@2.7.1 --check . --ignore-path .prettierignore --ignore-unknown || true
	@echo "=== Clippy (workspace, all targets) ==="
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "=== Semgrep ==="
	@semgrep scan --config auto --error .
	@echo "Quality gate passed"

fix:
	@echo "=== Format Fix (Rust) ==="
	@cargo fmt --all
	@echo "=== Format Fix (non-Rust) ==="
	@npx --yes prettier@2.7.1 --write . --ignore-path .prettierignore --ignore-unknown
	@echo "Formatting applied"

# ============================================================================
# Git Hooks
# ============================================================================

hooks:
	@echo "Installing pre-commit hooks..."
	@command -v pre-commit >/dev/null 2>&1 || { echo "Install pre-commit: pipx install pre-commit"; exit 1; }
	@pre-commit install --hook-type pre-commit --hook-type pre-push
	@echo "Hooks installed: pre-commit (check+fmt) + pre-push (clippy+semgrep)"

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
	@echo "  make build           Build debug binary"
	@echo "  make release         Build release binary (with embedded model)"
	@echo "  make install         Install CLI (~888MB with embedded model)"
	@echo "  make install-no-embed Install without model (needs CODESCRIBE_MODEL_PATH)"
	@echo "  make config          Edit ~/.codescribe/.env"
	@echo "  make bundle          Create CodeScribe.app bundle"
	@echo "  make install-app     Install to /Applications"
	@echo ""
	@echo "Release & Distribution:"
	@echo "  make dmg             Build DMG (ad-hoc signed)"
	@echo "  make dmg-signed      Build DMG (Developer ID signed)"
	@echo "  make release-full    Build + sign + notarize DMG (release-ready)"
	@echo "  make notarize        Notarize DMG with Apple"
	@echo "  make download-model  Download Whisper model from HF"
	@echo "  make download-e5     Download E5 embedder model from HF"
	@echo "  make download-embedder Download MiniLM embedder from HF"
	@echo "  make ensure-models   Download Whisper+MiniLM if missing from cache"
	@echo ""
	@echo "Run:"
	@echo "  make start           Start CodeScribe"
	@echo "  make stop            Stop CodeScribe"
	@echo "  make restart         Restart"
	@echo "  make status          Show status"
	@echo "  make logs            Show logs"
	@echo "  make logs-follow     Tail logs"
	@echo ""
	@echo "Version:"
	@echo "  make version         Show current version"
	@echo "  make bump-patch      Bump patch (0.5.1 -> 0.5.2)"
	@echo "  make bump-minor      Bump minor (0.5.1 -> 0.6.0)"
	@echo "  make bump-major      Bump major (0.5.1 -> 1.0.0)"
	@echo ""
	@echo "Quality:"
	@echo "  make lint            Run clippy + fmt check"
	@echo "  make format          Format Rust code"
	@echo "  make fix             Format all code (Rust + Prettier)"
	@echo "  make test            Run full test suite (incl. ignored real-API tests)"
	@echo "  make test-quick      Run tests without real-API calls"
	@echo "  make test-e2e        Run E2E tests (mock)"
	@echo "  make test-e2e-real   Run E2E tests with real API (needs LLM_*_API_KEY)"
	@echo "  make test-sse        Run SSE streaming tests (real API)"
	@echo "  make test-formatting Run AI formatting tests"
	@echo "  make test-all        Run full test suite"
	@echo "  make check           Verify formatting + clippy + semgrep (CI-safe)"
	@echo "  make hooks           Install pre-commit + pre-push hooks"

# ============================================================================
# Release & Distribution
# ============================================================================

dmg:
	@./scripts/build-dmg.sh

dmg-signed:
	@./scripts/build-dmg.sh --sign

release-full:
	@./scripts/build-dmg.sh --sign --notarize

notarize:
	@if ls CodeScribe_*.dmg 1> /dev/null 2>&1; then \
		DMG=$$(ls -t CodeScribe_*.dmg | head -1); \
		./scripts/notarize.sh "$$DMG"; \
	else \
		echo "No DMG found. Run 'make dmg-signed' first."; \
	fi

download-model:
	@./scripts/download-model.sh

download-e5:
	@./scripts/download-e5.sh

download-embedder:
	@./scripts/download-embedder.sh

ensure-models:
	@./scripts/ensure-models.sh
