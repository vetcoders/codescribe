# Codescribe - Pure Rust Build System
# Speech-to-text tray app for macOS

.PHONY: all build release release-codescribe release-codescribe-embedded release-qube install install-no-embed config bundle install-app \
        start stop restart status logs logs-follow \
        bump bump-patch bump-minor bump-major version \
        lint format test test-quick test-e2e test-e2e-real test-sse test-sse-release test-responses-live test-sse-heavy test-formatting test-all \
        demo demo-raw demo-assistive check semgrep fix clean help \
        dmg dmg-signed release-standard release-full release-dmgs notarize download-model download-e5 download-embedder ensure-models \
        hooks

SHELL := /bin/bash
VERSION_FILE := Cargo.toml
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)
ENV_LOAD := set -a; [ -f $$HOME/.codescribe/.env ] && source $$HOME/.codescribe/.env; set +a
# macOS: TCC tracks a stable code identity, not just bundle path. Prefer a stable
# Apple-issued signing identity automatically, and only fall back to ad-hoc when
# there is genuinely nothing usable in the keychain.
CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY := $(shell security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)"/\1/p' | head -n 1)
CODESCRIBE_DEVELOPER_ID_IDENTITY := $(shell security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)"/\1/p' | head -n 1)
CODESCRIBE_AUTO_CODESIGN_IDENTITY := $(if $(strip $(CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY)),$(strip $(CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY)),$(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)))
# Example:
#   CODESCRIBE_CODESIGN_IDENTITY="Apple Development: Your Name (TEAMID)" make install-app
CODESCRIBE_CODESIGN_IDENTITY ?= $(if $(CODESCRIBE_AUTO_CODESIGN_IDENTITY),$(CODESCRIBE_AUTO_CODESIGN_IDENTITY),-)
CODESCRIBE_APP_NAME ?= Codescribe
CODESCRIBE_DISPLAY_NAME ?= Codescribe
CODESCRIBE_BUNDLE_ID ?= com.vetcoders.codescribe
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

release-codescribe: ensure-models
	@echo "Building codescribe (release, embedded models: Silero + MiniLM + Whisper)..."
	@CODESCRIBE_EMBED_WHISPER=1 cargo build --release --bin codescribe

# Compatibility alias — embedding Whisper is now the default for release-codescribe.
# Kept so existing scripts / muscle memory keep working; NOT a separate public lane.
release-codescribe-embedded: release-codescribe

release-qube:
	@echo "Building qube-* (release, runtime model resolve from HF cache)..."
	@CODESCRIBE_NO_EMBED=1 cargo build --release --target-dir target-noembed --bin qube-daemon --bin qube-report

release: release-codescribe release-qube

install:
	@echo "Installing Codescribe (embedded models: Silero + MiniLM + Whisper)..."
	@./scripts/ensure-models.sh
	@CODESCRIBE_EMBED_WHISPER=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@$(MAKE) hooks
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"

install-no-embed:
	@echo "Installing Codescribe (DEV/RECOVERY: runtime Whisper fallback + no optional embedded support assets)..."
	@./scripts/ensure-models.sh
	@CODESCRIBE_NO_EMBED=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@$(MAKE) hooks
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"
	@echo "Note: Set CODESCRIBE_MODEL_PATH at runtime"

config:
	@mkdir -p ~/.codescribe
	@if [ ! -f ~/.codescribe/.env ]; then \
		cp .env.example ~/.codescribe/.env 2>/dev/null || echo "# Codescribe Config" > ~/.codescribe/.env; \
		echo "Created ~/.codescribe/.env"; \
	fi
	@$(EDITOR) ~/.codescribe/.env

bundle: ensure-models release
	@echo "Creating macOS app bundle..."
	@rm -rf bundle/$(CODESCRIBE_APP_NAME).app
	@mkdir -p bundle/$(CODESCRIBE_APP_NAME).app/Contents/{MacOS,Resources}
	@cp target/release/codescribe bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/
	@cp target-noembed/release/qube-daemon bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/ 2>/dev/null || true
	@cp target-noembed/release/qube-report bundle/$(CODESCRIBE_APP_NAME).app/Contents/MacOS/ 2>/dev/null || true
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
		'  <key>NSAppleEventsUsageDescription</key><string>Needed to activate the previously focused app and restore selection context.</string>' \
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
		echo "Codesigning ad-hoc (no stable signing identity found in keychain)."; \
		echo "NOTE: macOS Accessibility/Input Monitoring may need re-grant after reinstall."; \
		echo "TIP: add an Apple Development or Developer ID Application certificate, or set CODESCRIBE_CODESIGN_IDENTITY explicitly."; \
		codesign --force --deep --sign - --identifier $(CODESCRIBE_BUNDLE_ID) /Applications/$(CODESCRIBE_APP_NAME).app; \
	else \
		echo "Codesigning with stable identity: $(CODESCRIBE_CODESIGN_IDENTITY)"; \
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
	@echo "Codescribe started (logs: /tmp/codescribe.log)"

stop:
	@pkill -x codescribe 2>/dev/null || true
	@rm -f ~/.codescribe/codescribe.pid 2>/dev/null || true
	@echo "Stopped"

restart: stop
	@sleep 1
	@$(MAKE) start

status:
	@echo "=== Codescribe Status ==="
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
TEST_SSE_CARGO_JOBS ?= 2
TEST_SSE_PROFILE ?= debug
TEST_SSE_PROFILE_ARGS := $(if $(filter release,$(TEST_SSE_PROFILE)),--release,)

define TEST_SETUP
LOG=$(TEST_LOG); \
export CODESCRIBE_DISABLE_KEYCHAIN=1; \
echo "" >> "$$LOG"; \
echo "╔══════════════════════════════════════════════════════════╗" | tee -a "$$LOG"; \
echo "║  Codescribe Test Suite — $$(date '+%Y-%m-%d %H:%M:%S')           ║" | tee -a "$$LOG"; \
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
	set -o pipefail; \
	echo "=== SSE Streaming Tests ===" | tee -a "$$LOG"; \
	TEST_SSE_PROFILE="$(TEST_SSE_PROFILE)" CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" ./scripts/test-sse-preflight.sh 2>&1 | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" \
	cargo test --test e2e_sse_streaming $(TEST_SSE_PROFILE_ARGS) -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; \
	if [[ "$${CODESCRIBE_TEST_SSE_RESPONSES:-0}" == "1" ]]; then \
	  echo "=== Responses Live Chain/Resume Tests ===" | tee -a "$$LOG"; \
	  $(ENV_LOAD); CODESCRIBE_E2E_RESPONSES=1 CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" \
	  cargo test --test e2e_retry_responses -- --nocapture 2>&1 | tee -a "$$LOG"; \
	else \
	  echo "Skipping Responses Live Chain/Resume Tests (set CODESCRIBE_TEST_SSE_RESPONSES=1)." | tee -a "$$LOG"; \
	fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-sse-release:
	@CODESCRIBE_ALLOW_RELEASE_SSE=1 TEST_SSE_PROFILE=release $(MAKE) test-sse

test-responses-live:
	@CODESCRIBE_TEST_SSE_RESPONSES=1 $(MAKE) test-sse

test-sse-heavy:
	@CODESCRIBE_ALLOW_RELEASE_SSE=1 CODESCRIBE_TEST_SSE_RESPONSES=1 TEST_SSE_PROFILE=release $(MAKE) test-sse

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

semgrep:
	@semgrep scan --config auto --error --quiet .

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
	@pre-commit install --hook-type pre-commit --hook-type pre-push --hook-type commit-msg
	@echo "Hooks installed: pre-commit (check+fmt) + pre-push (clippy+semgrep) + commit-msg (provenance)"

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

# Help colors
HELP_C_CYAN   := \033[36m
HELP_C_GREEN  := \033[32m
HELP_C_YELLOW := \033[33m
HELP_C_RESET  := \033[0m

help:
	@printf '\n$(HELP_C_CYAN)%s$(HELP_C_RESET)\n' 'Codescribe - Speech-to-text (Pure Rust)'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'BUILD & INSTALL'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'build' 'Build debug binary'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release' 'Build release binary with embedded models (Silero + MiniLM + Whisper)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' 'Install CLI with embedded models (Silero + MiniLM + Whisper)'
	@printf '%s\n' '  make install-no-embed Install without optional embedded assets (needs CODESCRIBE_MODEL_PATH)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'config' 'Edit ~/.codescribe/.env'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'bundle' 'Create Codescribe.app bundle'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install-app' 'Install to /Applications'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'RELEASE & DISTRIBUTION'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'dmg' 'Build DMG (ad-hoc signed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'dmg-signed' 'Build DMG (Developer ID signed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-standard' 'Build + sign + notarize release DMG (embedded models)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-full' 'Compatibility alias for release-standard (embedded by default)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-dmgs' 'Build the notarized release DMG'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'notarize' 'Notarize DMG with Apple'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'download-model' 'Download Whisper model from HF'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'download-e5' 'Download E5 embedder model from HF'
	@printf '%s\n' '  make download-embedder Download MiniLM embedder from HF'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'ensure-models' 'Download Whisper+MiniLM if missing from cache'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'RUN'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'start' 'Start Codescribe'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'stop' 'Stop Codescribe'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'restart' 'Restart'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'status' 'Show status'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'logs' 'Show logs'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'logs-follow' 'Tail logs'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'VERSION'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'version' 'Show current version'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'bump-patch' 'Bump patch (0.5.1 -> 0.5.2)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'bump-minor' 'Bump minor (0.5.1 -> 0.6.0)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'bump-major' 'Bump major (0.5.1 -> 1.0.0)'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'QUALITY'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'lint' 'Run clippy + fmt check'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'format' 'Format Rust code'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'fix' 'Format all code (Rust + Prettier)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test' 'Run full test suite (incl. ignored real-API tests)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-quick' 'Run tests without real-API calls'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-e2e' 'Run E2E tests (mock)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-e2e-real' 'Run E2E tests with real API (needs LLM_*_API_KEY)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-sse' 'Run SSE streaming tests (real API)'
	@printf '%s\n' '  make test-formatting Run AI formatting tests'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-all' 'Run full test suite'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'check' 'Verify formatting + clippy + semgrep (CI-safe)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'semgrep' 'Run release security scan'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'hooks' 'Install pre-commit + pre-push + commit-msg hooks'

# ============================================================================
# Release & Distribution
# ============================================================================

dmg:
	@./scripts/build-dmg.sh

dmg-signed:
	@./scripts/build-dmg.sh --sign

release-standard:
	@./scripts/build-dmg.sh --sign --notarize

# Compatibility alias — the standard DMG now embeds Whisper by default, so it IS
# the real user artifact. `_full` no longer denotes a separate "real" build.
release-full: release-standard

release-dmgs: release-standard

notarize:
	@if ls Codescribe_*.dmg 1> /dev/null 2>&1; then \
		DMG=$$(ls -t Codescribe_*.dmg | head -1); \
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
