# CodeScribe - Pure Rust Build System
# Speech-to-text tray app for macOS

.PHONY: all build release install install-no-embed config bundle install-app \
        start stop restart status logs logs-follow \
        bump bump-patch bump-minor bump-major version \
        lint format test test-quick test-e2e test-e2e-real test-sse test-formatting test-all \
        demo demo-raw demo-assistive check fix clean help \
        dmg dmg-signed dmg-full notarize download-model download-e5 \
        hooks

SHELL := /bin/bash
VERSION_FILE := Cargo.toml
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)
ENV_LOAD := set -a; [ -f $$HOME/.codescribe/.env ] && source $$HOME/.codescribe/.env; set +a
E2E_ENV_EXAMPLE := .env.example
E2E_ENV_FILE := ./.env.e2e
TEST_BUILD_JOBS ?= 2
ENV_LOAD_E2E := set -a; [ -f $(E2E_ENV_FILE) ] && source $(E2E_ENV_FILE); set +a
E2E_ENV_GEN := ./scripts/validate-envs.sh --env-example --env-example-path $(E2E_ENV_EXAMPLE) --emit-e2e-env $(E2E_ENV_FILE)

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
	@echo "Installing CodeScribe (embedding optional via CODESCRIBE_EMBED_*)..."
	@cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@echo "Installed: codescribe $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"

install-no-embed:
	@echo "Installing CodeScribe (no embedded model)..."
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
# Quality
# ============================================================================

format:
	@cargo fmt

lint:
	@echo "=== Format Check ==="
	@cargo fmt -- --check
	@echo "=== Clippy ==="
	@cargo clippy --workspace -- -D warnings

test:
	@echo "=== Tests (workspace, all incl. real API) ==="
	@$(ENV_LOAD); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test --workspace --all-targets -- --include-ignored --nocapture

test-quick:
	@echo "=== Tests (quick, no real API) ==="
	@$(ENV_LOAD); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test --workspace --all-targets -- --nocapture

test-e2e:
	@echo "=== E2E Tests (mock) ==="
	@$(E2E_ENV_GEN)
	@$(ENV_LOAD_E2E); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test e2e --release -- --nocapture

test-e2e-real:
	@echo "=== E2E Tests (real API) ==="
	@echo "Requires: LLM_API_KEY, LLM_ASSISTIVE_API_KEY"
	@$(E2E_ENV_GEN)
	@$(ENV_LOAD_E2E); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test e2e --release -- --ignored --nocapture

test-sse:
	@echo "=== SSE Streaming Tests ==="
	@$(E2E_ENV_GEN)
	@$(ENV_LOAD_E2E); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test e2e_sse --release -- --ignored --nocapture

test-formatting:
	@echo "=== AI Formatting Tests ==="
	@$(ENV_LOAD); $(APPLY_TEST_LLM); CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test formatting --release -- --nocapture

test-all:
	@echo "=== Full Test Suite ==="
	@$(MAKE) test
	@echo "Done."

test-roundtrip:
	@echo "=== Round-Trip Tests (TTS→STT→Embeddings) ==="
	@echo "Tests actual embedded models pipeline integrity"
	@$(E2E_ENV_GEN)
	@$(ENV_LOAD_E2E); CODESCRIBE_E2E_ROUNDTRIP=1 CARGO_BUILD_JOBS=$(TEST_BUILD_JOBS) cargo test --test e2e_round_trip --release -- --nocapture

demo:
	@echo "=== Full Pipeline Demo ==="
	@cargo run --release --example demo_full_pipeline -- $(AUDIO)

demo-roundtrip:
	@echo "=== Round-Trip Interactive Demo ==="
	@cargo run --release --example roundtrip_live -- --interactive

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
	@git ls-files --cached --exclude-standard | xargs npx --yes prettier@2.7.1 --check --ignore-path .prettierignore --ignore-unknown || true
	@echo "=== Clippy (workspace, all targets) ==="
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "=== Semgrep ==="
	@semgrep scan --config auto --error .
	@echo "Quality gate passed"

fix:
	@echo "=== Format Fix (Rust) ==="
	@cargo fmt --all
	@echo "=== Format Fix (non-Rust) ==="
	@git ls-files --cached --exclude-standard | xargs npx --yes prettier@2.7.1 --write --ignore-path .prettierignore --ignore-unknown
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
	@echo "  make release         Build release binary (embedding optional via env)"
	@echo "  make install         Install CLI (embedding optional via env)"
	@echo "  make install-no-embed Install without model (needs CODESCRIBE_MODEL_PATH)"
	@echo "  make config          Edit ~/.codescribe/.env"
	@echo "  make bundle          Create CodeScribe.app bundle"
	@echo "  make install-app     Install to /Applications"
	@echo ""
	@echo "Release & Distribution:"
	@echo "  make dmg             Create DMG from existing .app (unsigned)"
	@echo "  make dmg-signed      Build .app + DMG + sign (NO notarize)"
	@echo "  make dmg-full        Build .app + DMG + sign + notarize"
	@echo "  make notarize        Notarize existing DMG (NOTARY_PROFILE required)"
	@echo ""
	@echo "Release env vars:"
	@echo "  SIGN_IDENTITY=...        Codesign identity (Developer ID Application: ...)"
	@echo "  NOTARY_PROFILE=...       notarytool profile name"
	@echo "  BUNDLE_WHISPER=1          bundle local Whisper into .app"
	@echo "  BUNDLE_FALLBACK_GIT=1     allow fallback git clone if no local model"
	@echo "  make download-model  Download Whisper model from HF"
	@echo "  make download-e5     Download E5 embedder model from HF"
	@echo "  make download-silero-vad  Download Silero VAD model from HF"
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
	@./packaging/create_dmg.sh

dmg-signed:
	@SIGN_IDENTITY="$(SIGN_IDENTITY)" NOTARY_PROFILE="$(NOTARY_PROFILE)" \
		BUNDLE_WHISPER="$(BUNDLE_WHISPER)" BUNDLE_FALLBACK_GIT="$(BUNDLE_FALLBACK_GIT)" \
		./packaging/release.sh --no-notary

dmg-full:
	@SIGN_IDENTITY="$(SIGN_IDENTITY)" NOTARY_PROFILE="$(NOTARY_PROFILE)" \
		BUNDLE_WHISPER="$(BUNDLE_WHISPER)" BUNDLE_FALLBACK_GIT="$(BUNDLE_FALLBACK_GIT)" \
		./packaging/release.sh

notarize:
	@if ls CodeScribe_*.dmg 1> /dev/null 2>&1; then \
		DMG=$$(ls -t CodeScribe_*.dmg | head -1); \
		NOTARY_PROFILE="$(NOTARY_PROFILE)" ./scripts/notarize.sh "$$DMG"; \
	else \
		echo "No DMG found. Run 'make dmg-signed' first."; \
	fi

download-model:
	@./scripts/download-model.sh

download-e5:
	@./scripts/download-e5.sh

download-silero-vad:
	@./scripts/download-silero-vad.sh
