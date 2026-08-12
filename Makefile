# Codescribe - Build System
# Speech-to-text for macOS: SwiftUI front-end (macos/) over a Rust engine
# (core/ + app/ lib) bridged with UniFFI (bridge/ = codescribe-ffi).
# The user-facing app is built by `make app` (xcodebuild); the Rust side no
# longer ships a standalone `codescribe` tray binary.

.PHONY: all build release release-codescribe release-codescribe-embedded release-qube app app-bindings install install-no-embed config install-app \
        start stop restart status logs logs-follow \
        bump bump-patch bump-minor bump-major version \
        lint format test test-quick test-e2e test-e2e-real test-sse test-sse-release test-responses-live test-sse-heavy test-formatting test-all \
        test-engine test-engine-apple test-engine-candle test-teacher \
        demo demo-raw demo-assistive check verify semgrep fix clean help \
        dist-preflight dist-preflight-signed verify-canaries smoke-canaries \
        dmg dmg-signed release-standard release-full release-dmgs notarize verify-dmg download-model download-e5 download-embedder ensure-models \
        hooks

SHELL := /bin/bash
VERSION_FILE := Cargo.toml
EDITOR ?= $(shell command -v code || command -v nvim || command -v vim || echo nano)
# Operator tests may source the daily dotenv for real-API credentials, but the
# harness owns its data directory. Preserve that process-wide isolation across
# the source so an operator CODESCRIBE_DATA_DIR cannot redirect tests back into
# a persistent or production tree.
ENV_LOAD := CODESCRIBE_TEST_DATA_DIR_GUARD=$$CODESCRIBE_DATA_DIR; set -a; [ -f $$HOME/.codescribe/.env ] && source $$HOME/.codescribe/.env; set +a; export CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR_GUARD"; unset CODESCRIBE_TEST_DATA_DIR_GUARD
# macOS: TCC tracks a stable code identity, not just bundle path. Prefer a stable
# Apple-issued signing identity automatically, and only fall back to ad-hoc when
# there is genuinely nothing usable in the keychain.
CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY := $(shell security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)"/\1/p' | head -n 1)
CODESCRIBE_DEVELOPER_ID_IDENTITY := $(shell security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)"/\1/p' | head -n 1)
CODESCRIBE_AUTO_CODESIGN_IDENTITY := $(if $(strip $(CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY)),$(strip $(CODESCRIBE_APPLE_DEVELOPMENT_IDENTITY)),$(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)))
# Example:
#   CODESCRIBE_CODESIGN_IDENTITY="Apple Development: Your Name (TEAMID)" make install-app
CODESCRIBE_CODESIGN_IDENTITY ?= $(if $(CODESCRIBE_AUTO_CODESIGN_IDENTITY),$(CODESCRIBE_AUTO_CODESIGN_IDENTITY),-)
# Distribution artifacts (signed DMG + notarization) require Developer ID, not
# the TCC-friendly Apple Development identity preferred above for local
# installs. Recipes must pass this into build-dmg.sh explicitly — make vars are
# not exported to recipe children, which is exactly how `make release-standard`
# used to reach --sign with an empty identity.
CODESCRIBE_DIST_CODESIGN_IDENTITY ?= $(if $(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),$(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),$(CODESCRIBE_CODESIGN_IDENTITY))
# The production licence verification key has the same resolution problem the
# signing identity above already solved: CI passes it as a repository variable
# (release.yml), a local checkout had no source at all, and core/build.rs only
# discovers it is missing ~2 minutes into cargo — as a bare panic that names
# neither make nor where to get the key. Resolve it from the canonical secrets
# file so distribution targets are self-sufficient.
#
# Deliberately a SEPARATE variable from CODESCRIBE_LICENSE_PUBLIC_KEY_HEX:
# install-app and scripts/smoke-macos27.sh disarm distribution keying with
# `env -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX`, and that must keep working — a
# plain `?=` fallback here would silently re-arm the production key in a local
# build that deliberately wants the development verifier.
CODESCRIBE_LICENSE_PUBLIC_KEY_FILE ?= $(HOME)/.vibecrafted/secrets/codescribe/license-public.hex
CODESCRIBE_DIST_LICENSE_KEY = $(if $(CODESCRIBE_LICENSE_PUBLIC_KEY_HEX),$(CODESCRIBE_LICENSE_PUBLIC_KEY_HEX),$(shell cat $(CODESCRIBE_LICENSE_PUBLIC_KEY_FILE) 2>/dev/null | tr -d '[:space:]'))
# Sparkle's update-verification public key has the same missing-local-source
# problem: release.yml supplies SPARKLE_ED_PUBLIC_KEY as a repository variable,
# macos/project.yml substitutes it into SUPublicEDKey, and
# scripts/verify-dmg-payload.sh refuses a bundle whose key is empty ("Sparkle
# would reject every update"). A local `make release-standard` had no way to
# supply it, so a locally cut release failed the gate at the very last check —
# after codesigning, notarisation and stapling had already been paid for.
CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE ?= $(HOME)/.vibecrafted/secrets/codescribe/sparkle-public.b64
CODESCRIBE_DIST_SPARKLE_KEY = $(if $(SPARKLE_ED_PUBLIC_KEY),$(SPARKLE_ED_PUBLIC_KEY),$(shell cat $(CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE) 2>/dev/null | tr -d '[:space:]'))
CODESCRIBE_APP_NAME ?= Codescribe
CODESCRIBE_DISPLAY_NAME ?= Codescribe
# SwiftUI app build profile for `make app` / `make app-bindings`
# (debug|local-release|release). `release` is distribution-only and requires
# the operator-owned production license verification key.
PROFILE ?= debug
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

# Slim public default: Silero VAD + MiniLM. Whisper is runtime/cache/Settings download.
# Do NOT set CODESCRIBE_EMBED_WHISPER here — that is the fat experimental SKU only.
release-codescribe: dist-preflight
	@echo "Building codescribe-ffi (release dylib, embedded: Silero + MiniLM; Whisper runtime)..."
	@echo "  The app front-end is no longer a Rust bin; this builds the UniFFI bridge dylib."
	@echo "  Produce the runnable SwiftUI app with: make app PROFILE=release"
	@echo "  Fat Whisper embed: make release-codescribe-embedded"
	@CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 env -u CODESCRIBE_EMBED_WHISPER -u CODESCRIBE_NO_EMBED cargo build --release -p codescribe-ffi

# Optional fat SKU / offline curiosity: bake Whisper into the dylib (~1GB+).
# Not the daily release path. Pair with `make release-full` for a _full DMG.
release-codescribe-embedded: dist-preflight ensure-models
	@echo "Building codescribe-ffi (FAT: Silero + MiniLM + Whisper embedded)..."
	@CODESCRIBE_EMBED_WHISPER=1 CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 cargo build --release -p codescribe-ffi

# ── SwiftUI app (macos/) via the codescribe-ffi UniFFI bridge ────────────────
# Full verified pipeline: cargo (ffi dylib) → uniffi-bindgen → xcodegen → xcodebuild.
# `app-bindings` stops after xcodegen (no Xcode needed) for fast Rust-side iteration.
app:
	@echo "Building Codescribe.app (SwiftUI, PROFILE=$(PROFILE))..."
	@./scripts/build-app.sh $(PROFILE)

app-bindings:
	@echo "Regenerating UniFFI Swift bindings + Xcode project (PROFILE=$(PROFILE), no xcodebuild)..."
	@SKIP_XCODEBUILD=1 ./scripts/build-app.sh $(PROFILE)

release-qube: dist-preflight
	@echo "Building qube-* (release, runtime model resolve from HF cache)..."
	@CODESCRIBE_NO_EMBED=1 CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 cargo build --release --target-dir target-noembed --bin qube-daemon --bin qube-report

release: release-codescribe release-qube

install:
	@echo "Installing qube tools + codescribe CLI (slim: Silero + MiniLM; Whisper from cache / Settings)..."
	@echo "Local install uses the development license verifier — same contract as install-app."
	@./scripts/download-embedder.sh || true
	@env -u CODESCRIBE_EMBED_WHISPER -u CODESCRIBE_NO_EMBED -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX \
	 CODESCRIBE_LOCAL_INSTALL=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@$(MAKE) hooks
	@echo "Installed: qube tools $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"
	@echo "Note: Whisper is not embedded — download via Settings → Dictation or make download-model"

install-no-embed:
	@echo "Installing qube tools (DEV/RECOVERY: no optional embeds; runtime paths only)..."
	@env -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX \
	 CODESCRIBE_NO_EMBED=1 CODESCRIBE_LOCAL_INSTALL=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@$(MAKE) hooks
	@echo "Installed: qube tools $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"
	@echo "Note: Set CODESCRIBE_MODEL_PATH at runtime if Whisper is needed"

config:
	@mkdir -p ~/.codescribe
	@if [ ! -f ~/.codescribe/.env ]; then \
		cp .env.example ~/.codescribe/.env 2>/dev/null || echo "# Codescribe Config" > ~/.codescribe/.env; \
		echo "Created ~/.codescribe/.env"; \
	fi
	@$(EDITOR) ~/.codescribe/.env


install-app:
	@echo "Building $(CODESCRIBE_APP_NAME).app (SwiftUI, optimized local profile) via scripts/build-app.sh ..."
	@echo "Local install uses the development license verifier; CODESCRIBE_LICENSE_PUBLIC_KEY_HEX is reserved for distribution builds."
	@env -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX $(MAKE) --no-print-directory app PROFILE=local-release
	@APP_SRC="macos/build/Build/Products/Release/Codescribe.app"; \
	if [ ! -d "$$APP_SRC" ]; then \
		echo "Build product missing: $$APP_SRC — 'make app PROFILE=local-release' did not produce the app."; \
		exit 1; \
	fi; \
	echo "Installing to /Applications ..."; \
	mkdir -p /Applications; \
	rsync -a --delete "$$APP_SRC/" "/Applications/$(CODESCRIBE_APP_NAME).app/"
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
	@open -a "$(CODESCRIBE_APP_NAME)" 2>/dev/null \
		|| open "/Applications/$(CODESCRIBE_APP_NAME).app" 2>/dev/null \
		|| { echo "$(CODESCRIBE_APP_NAME).app not found — build it with 'make app' or install with 'make install-app'."; exit 1; }
	@echo "$(CODESCRIBE_APP_NAME) launched"

stop:
	@pkill -x "$(CODESCRIBE_APP_NAME)" 2>/dev/null || true
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

# ── GATE LEDGER ─────────────────────────────────────────────────────────────
#
# What green means here. A verification command is authoritative ONLY for what
# it executes, and no surface may cite it as proof of something it does not run.
# This block is the single place that says which is which; `check` enforces it
# through scripts/validate-gates.sh, and tests/gate_registry.rs runs the same
# validator under `cargo test` so the classification reaches CI — CI never
# invokes a Makefile quality target itself.
#
# Adding a verification target without a row here fails the gate. So does
# wiring one into .github/workflows/ without flipping its `ci=` field, and so
# does claiming CI coverage no workflow provides. Both directions are checked.
#
#   class=static     no tests executed — format, lint, security scanning only
#   class=hermetic   runs from a clean checkout: no operator dotenv, no private
#                    corpus, no GUI, no API keys, no audio device, no Xcode
#   class=operator   needs this host: ~/.codescribe/.env, real API keys, mic or
#                    BlackHole, the private fixture corpus, Xcode, or a human
#   ci=yes|no        whether a workflow in .github/workflows/ invokes THIS TARGET
#                    BY NAME. Narrow on purpose, because it is the only claim a
#                    script can check without guessing. It does NOT mean "this
#                    check never runs in CI": release.yml reaches the DMG payload
#                    check by calling scripts/verify-dmg-payload.sh directly, so
#                    `verify-dmg` is honestly ci=no while its check does run.
#                    Targets reached transitively through $(MAKE) inside another
#                    recipe are likewise not followed. Say the rest in the reach
#                    text — a field that quietly widened its meaning would be the
#                    same failure this ledger exists to stop.
#
# `make verify` is the one hermetic gate, and it is literally what CI runs —
# not a second recipe that resembles it. Everything below class=operator is a
# bench instrument: real proof, host-local, never a merge gate.
#
# gate: check class=static ci=no -- cargo fmt, prettier, clippy, semgrep, validate-envs, validate-gates; executes ZERO tests
# gate: lint class=static ci=no -- cargo fmt --check + clippy on the workspace; no tests
# gate: semgrep class=static ci=no -- semgrep scan --config auto (semgrep.yml runs semgrep directly, not this target)
# gate: verify class=hermetic ci=yes -- the workspace test set + doctests + env registry + this ledger; the command rust.yml runs
# gate: verify-canaries class=hermetic ci=no -- claim-vs-execution canaries that read repo files only (scripts/canaries.sh); each row is born from a named incident
# gate: verify-swift-format class=static ci=no -- swift-format lint --strict over macos/Codescribe + macos/CodescribeTests; skips the generated UniFFI binding; no Swift tests (that is test-swift)
# gate: smoke-canaries class=operator ci=no -- verify-canaries + host rows: dist inputs, appcast feed, live-store purity, Sparkle key parity (scripts/canaries.sh --host)
# gate: verify-dmg class=operator ci=no -- fail-closed payload check against an already-built DMG; release.yml runs the same check via scripts/verify-dmg-payload.sh, not via this target
# gate: test class=operator ci=no -- workspace tests + #[ignore] real-API tests + STT pipeline; sources ~/.codescribe/.env and opens Console
# gate: test-quick class=operator ci=no -- workspace tests only, but still sources ~/.codescribe/.env and opens Console
# gate: test-all class=operator ci=no -- test + ignored + STT pipeline + SSE streaming; needs LLM keys
# gate: test-e2e class=operator ci=no -- e2e tests in release profile; sources the operator dotenv
# gate: test-e2e-real class=operator ci=no -- e2e against real LLM APIs; needs LLM_API_KEY and LLM_ASSISTIVE_API_KEY
# gate: test-sse class=operator ci=no -- live SSE streaming against a real endpoint
# gate: test-sse-release class=operator ci=no -- test-sse in the release profile
# gate: test-sse-heavy class=operator ci=no -- test-sse release + Responses chain/resume
# gate: test-responses-live class=operator ci=no -- Responses live chain/resume against a real endpoint
# gate: test-formatting class=operator ci=no -- AI formatting tests against a real LLM
# gate: test-engine class=operator ci=no -- live-assembly and final-boundary unit lanes with output
# gate: test-engine-apple class=operator ci=no -- Apple live engine over BlackHole; needs the private fixture corpus
# gate: test-engine-apple-channel class=operator ci=no -- Apple channel path from WAV; needs the private fixture corpus
# gate: test-engine-candle class=operator ci=no -- candle/Metal engine lane; needs local models
# gate: test-engine-parity class=operator ci=no -- Layer 0 parity bar vs the Apple reference; private corpus, host-local bench
# gate: test-engine-parity-layered class=operator ci=no -- Layer 1 parity arm judged on structure; private corpus, host-local bench
# gate: test-engine-parity-both class=operator ci=no -- runs both parity arms and prints the delta
# gate: test-teacher class=operator ci=no -- teacher CLI proof run, writes an HTML report
# gate: test-swift class=operator ci=no -- SwiftUI suite + Apple phrase-restart Rust/Swift lockstep self-test; needs Xcode and built ffi/bridge binaries
# gate: smoke-macos27 class=operator ci=no -- host smoke after an OS/Xcode bump; operator-only rows report SKIP
#
# ─────────────────────────────────────────────────────────────────────────────

format:
	@cargo fmt

lint:
	@echo "=== Format Check ==="
	@cargo fmt -- --check
	@echo "=== Clippy ==="
	@cargo clippy --workspace -- -D warnings

# The Swift side of the app had no format gate at all while `lint` covered only
# Rust, so 100 of 100 sources drifted. Two things this recipe does NOT copy from
# the sibling repo it was transplanted from (vetcoders/pensieve, `make lint`):
#
#   1. `--strict` is mandatory. `swift-format lint` without it exits 0 no matter
#      how many violations it prints, so a gate built on the bare command is a
#      gate that can never fail — measured here on 2026-08-12 with swift-format
#      6.3.0: bare exit 0, --strict exit 1 on the same file.
#   2. The generated UniFFI binding is excluded. It is regenerated by
#      `make app-bindings` from the Rust bridge, so formatting it is both futile
#      and a diff-churn source; the sibling repo excludes its own binding the
#      same way.
#
# Diagnostics go to the terminal directly, not to stdout/stderr — redirecting
# this command yields an empty file while the console still fills. Judge it by
# the exit code; that is what survives a pipe and a CI log.
SWIFT_FORMAT_ROOTS := macos/Codescribe macos/CodescribeTests
SWIFT_FORMAT_EXCLUDE := -path '*/Bridge/codescribe_ffi.swift'

.PHONY: verify-swift-format format-swift
verify-swift-format:
	@if ! command -v swift-format >/dev/null 2>&1; then \
		echo "verify-swift-format: swift-format is required (brew install swift-format)"; \
		exit 1; \
	fi
	@find $(SWIFT_FORMAT_ROOTS) -name '*.swift' ! $(SWIFT_FORMAT_EXCLUDE) -print0 \
		| xargs -0 swift-format lint --strict

format-swift:
	@if ! command -v swift-format >/dev/null 2>&1; then \
		echo "format-swift: swift-format is required (brew install swift-format)"; \
		exit 1; \
	fi
	@find $(SWIFT_FORMAT_ROOTS) -name '*.swift' ! $(SWIFT_FORMAT_EXCLUDE) -print0 \
		| xargs -0 swift-format format --in-place
	@echo "format-swift: applied; re-run 'make verify-swift-format' to confirm"

TEST_LOG := /tmp/codescribe-tests.log
SWIFT_TEST_LOG := /tmp/codescribe-swift-tests.log
TEST_SSE_CARGO_JOBS ?= 2
TEST_SSE_PROFILE ?= debug
TEST_SSE_PROFILE_ARGS := $(if $(filter release,$(TEST_SSE_PROFILE)),--release,)

define TEST_DATA_DIR_SETUP
CODESCRIBE_TEST_TMP_ROOT="$${TMPDIR:-/tmp}"; \
CODESCRIBE_TEST_TMP_ROOT="$${CODESCRIBE_TEST_TMP_ROOT%/}"; \
if [[ -z "$$CODESCRIBE_TEST_TMP_ROOT" ]]; then CODESCRIBE_TEST_TMP_ROOT=/tmp; fi; \
CODESCRIBE_TEST_DATA_DIR="$$(mktemp -d "$$CODESCRIBE_TEST_TMP_ROOT/codescribe-test-data.XXXXXX")" || { \
  echo "test-data-dir: mktemp failed under $$CODESCRIBE_TEST_TMP_ROOT" >&2; \
  exit 1; \
}; \
export CODESCRIBE_DATA_DIR="$$CODESCRIBE_TEST_DATA_DIR"; \
cleanup_codescribe_test_data_dir() { \
  isolated_log="$$CODESCRIBE_TEST_DATA_DIR/logs/codescribe.log"; \
  if [[ -f "$$isolated_log" ]]; then \
    isolated_bytes="$$(wc -c < "$$isolated_log" | tr -d ' ')"; \
    echo "test-data-dir: isolated-log=$$isolated_log bytes=$$isolated_bytes"; \
  else \
    echo "test-data-dir: isolated-log=none root=$$CODESCRIBE_TEST_DATA_DIR"; \
  fi; \
  case "$$CODESCRIBE_TEST_DATA_DIR" in \
    "$$CODESCRIBE_TEST_TMP_ROOT"/codescribe-test-data.*) \
      rm -rf -- "$$CODESCRIBE_TEST_DATA_DIR"; \
      echo "test-data-dir: cleaned=$$CODESCRIBE_TEST_DATA_DIR"; \
      ;; \
    *) \
      echo "test-data-dir: refusing unsafe cleanup: $$CODESCRIBE_TEST_DATA_DIR" >&2; \
      return 1; \
      ;; \
  esac; \
}; \
trap cleanup_codescribe_test_data_dir EXIT; \
echo "test-data-dir: created=$$CODESCRIBE_TEST_DATA_DIR"
endef

define TEST_SETUP
$(TEST_DATA_DIR_SETUP); \
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
	set -o pipefail; \
	echo "=== Tests (workspace) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "=== Tests (ignored / real API) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "=== Full Pipeline (STT) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); CODESCRIBE_E2E_STT=1 \
	cargo test --test e2e_full_pipeline -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-quick:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== Tests (quick, no real API) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-e2e:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== E2E Tests (mock) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e --release -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-e2e-real:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== E2E Tests (real API) ===" | tee -a "$$LOG"; \
	echo "Requires: LLM_API_KEY, LLM_ASSISTIVE_API_KEY" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e --release -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

test-sse:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== SSE Streaming Tests ===" | tee -a "$$LOG"; \
	TEST_SSE_PROFILE="$(TEST_SSE_PROFILE)" CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" ./scripts/test-sse-preflight.sh 2>&1 | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" \
	cargo test --test e2e_sse_streaming $(TEST_SSE_PROFILE_ARGS) -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	if [[ "$${CODESCRIBE_TEST_SSE_RESPONSES:-0}" == "1" ]]; then \
	  echo "=== Responses Live Chain/Resume Tests ===" | tee -a "$$LOG"; \
	  $(ENV_LOAD); CODESCRIBE_E2E_RESPONSES=1 CARGO_BUILD_JOBS="$(TEST_SSE_CARGO_JOBS)" \
	  cargo test --test e2e_retry_responses -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	  if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
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
	set -o pipefail; \
	echo "=== AI Formatting Tests ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test formatting --release -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

# ── Core engine (freezed+append / Apple live multi-utterance) ────────────────
# Private fixtures live OUTSIDE the repo (real operator speech, deprivatized
# twice — .gitignore keeps tests/assets/data_assets empty by design). Resolve
# through the documented order: CODESCRIBE_DATA_ASSETS → ~/.codescribe/data_assets
# → the in-repo drop dir (tests/assets/data_assets/README.md). Hardcoding the
# third tier is what left every ENGINE_* target dead on a populated host.
DATA_ASSETS_DIR := $(shell ./scripts/lib/data-assets.sh dir)
# Clip for STT engine e2e (mic-sim → transcription_session). Override:
#   make test-engine-apple ENGINE_CLIP=~/.codescribe/data_assets/01_no-to-dobra.wav
#   make test-engine-apple ENGINE_ALL_CLIPS=1
ENGINE_CLIP ?= $(DATA_ASSETS_DIR)/02_kubernetes-wymaga-konfiguracji.wav
ENGINE_ALL_CLIPS ?= 0
ENGINE_BRIDGE ?= target/release/codescribe-stt-bridge
# Pin host triple so bridge binaries do not inherit the builder's macOS
# version (measured: host macosx28.0 / minos 28.0 on a 27 beta machine).
# Keep in lockstep with the SpeechAnalyzer / SFSpeech surface we ship against.
ENGINE_BRIDGE_TARGET ?= arm64-apple-macos26.0
# Minimal .app wrapper: TCC grants privacy prompts to bundles, not loose binaries.
ENGINE_BRIDGE_APP ?= target/release/CodescribeSTTBridge.app
# Live verbose: session/STT tracing during the long Apple/Candle run.
# Override quieter: ENGINE_RUST_LOG=warn make test-engine-apple
ENGINE_RUST_LOG ?= info,codescribe_core=info,codescribe=info

# Run cargo test with a PTY so stderr/stdout stay line-buffered through `tee`
# (plain pipe fully buffers → "running for over 60s" then silence for minutes).
define ENGINE_CARGO_TEST_LIVE
if command -v stdbuf >/dev/null 2>&1; then \
  stdbuf -oL -eL cargo test --test e2e_overlay_delivery_parity e2e_file_audio_as_mic_overlay_and_delivery_parity -- --nocapture 2>&1 | stdbuf -oL tee -a "$$LOG"; \
elif command -v script >/dev/null 2>&1; then \
  script -q /dev/null cargo test --test e2e_overlay_delivery_parity e2e_file_audio_as_mic_overlay_and_delivery_parity -- --nocapture 2>&1 | tee -a "$$LOG"; \
else \
  cargo test --test e2e_overlay_delivery_parity e2e_file_audio_as_mic_overlay_and_delivery_parity -- --nocapture 2>&1 | tee -a "$$LOG"; \
fi; test_rc=$${PIPESTATUS[0]}
endef

# Fast: pure assembly + stream-floor + always-on e2e contracts (no STT / no Apple).
test-engine:
	@echo "=== Core engine (unit + always-on contracts, no STT) ==="
	@cargo test -p codescribe-core --lib live_assembly -- --nocapture
	@cargo test -p codescribe-core --lib apply_final_boundary -- --nocapture
	@cargo test --test e2e_overlay_delivery_parity -- --nocapture
	@echo "OK — freezed+append + single-final-tail fail bar green."
	@echo "Apple live multi-utterance (slow):  make test-engine-apple"
	@echo "Candle live multi-utterance:        make test-engine-candle"

# Ensure Apple STT bridge binary exists (virtual-mic / AudioBuffer path).
# The Info.plist section is REQUIRED, not cosmetic: TCC crashes a process that
# asks for Speech Recognition without a usage description
# (__TCC_CRASHING_DUE_TO_PRIVACY_VIOLATION__), so without it the standalone
# bridge can never be authorized — and every engine test reports
# speech_auth_not_determined forever.
#
# Signing identity matters for TCC persistence: an AD-HOC signature keys the
# grant to the cdhash, so EVERY rebuild invalidates it (measured 2026-07-25 —
# the grind loop would need a re-auth click per rebuild). A real identity
# (Developer ID) keys it to the certificate's designated requirement, which is
# stable across rebuilds. Prefer it when present; fall back to ad-hoc.
BRIDGE_SIGN_ID ?= $(if $(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),$(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),-)
$(ENGINE_BRIDGE): core/stt/apple_stt/codescribe-stt-bridge.swift core/stt/apple_stt/bridge-Info.plist
	@echo "Building codescribe-stt-bridge → $(ENGINE_BRIDGE) (-target $(ENGINE_BRIDGE_TARGET))"
	@mkdir -p $(dir $(ENGINE_BRIDGE))
	@swiftc -O -target $(ENGINE_BRIDGE_TARGET) -o $(ENGINE_BRIDGE) core/stt/apple_stt/codescribe-stt-bridge.swift \
		-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist \
		-Xlinker core/stt/apple_stt/bridge-Info.plist
	@codesign --force --sign "$(BRIDGE_SIGN_ID)" --identifier com.vetcoders.codescribe.stt-bridge $(ENGINE_BRIDGE) 2>/dev/null || \
		{ codesign --force --sign - --identifier com.vetcoders.codescribe.stt-bridge $(ENGINE_BRIDGE) 2>/dev/null; \
		  echo "  (ad-hoc signed — TCC grant will NOT survive rebuilds)"; }
	@# TCC attributes a privacy prompt to a BUNDLE, not to a loose executable: an
	@# embedded __info_plist alone still aborts the process. Mirroring the binary
	@# into a minimal .app gives it a bundle identity, so `make engine-auth` can
	@# raise the dialog. Inside Codescribe.app the bridge inherits the app grant
	@# and this wrapper is unused.
	@mkdir -p $(ENGINE_BRIDGE_APP)/Contents/MacOS
	@cp core/stt/apple_stt/bridge-Info.plist $(ENGINE_BRIDGE_APP)/Contents/Info.plist
	@cp $(ENGINE_BRIDGE) $(ENGINE_BRIDGE_APP)/Contents/MacOS/codescribe-stt-bridge
	@codesign --force --sign "$(BRIDGE_SIGN_ID)" --identifier com.vetcoders.codescribe.stt-bridge $(ENGINE_BRIDGE_APP) 2>/dev/null || \
		codesign --force --sign - --identifier com.vetcoders.codescribe.stt-bridge $(ENGINE_BRIDGE_APP) 2>/dev/null || true

# One-time: raise the macOS Speech Recognition dialog for the standalone bridge.
# Needed before `make test-engine-apple` or scripts/e2e-blackhole-dictation.sh
# when the bridge runs outside Codescribe.app (inside the app it inherits the
# app's grant).
.PHONY: engine-auth
engine-auth: $(ENGINE_BRIDGE)
	@echo "Requesting Speech Recognition authorization (approve the system dialog)…"
	@# CODESCRIBE_BRIDGE_DISCLAIM=1 makes the bridge re-exec itself with the
	@# posix_spawn responsibility-disclaim attribute, so IT is the responsible
	@# process TCC evaluates — not the terminal. (`open -a` does NOT achieve
	@# this: launched from a terminal-descended process, the terminal stays the
	@# responsible process and the request aborts. Measured 2026-07-25.)
	@CODESCRIBE_BRIDGE_DISCLAIM=1 $(ENGINE_BRIDGE_APP)/Contents/MacOS/codescribe-stt-bridge request_auth pl-PL || true
	@echo "Status:"
	@printf '{"protocolVersion":1,"command":"probe","locale":"pl-PL","audioPath":null,"allowDownload":false}\n' \
		| CODESCRIBE_BRIDGE_DISCLAIM=1 $(ENGINE_BRIDGE_APP)/Contents/MacOS/codescribe-stt-bridge

# Reversed-TDD parity bar: our capture path must reproduce the SYSTEM Apple
# live output for the same audio (tests/assets/data_assets/README.md). RED
# until streaming bridge v2 lands — that is the point, not a flake. Every run
# prints token similarity + a word-level diff for the grinding loop.
#
# LANE PINNED, not inherited. `CODESCRIBE_LAYERED_TRANSCRIPTION` is a power-user
# key (not promoted to settings.json), so `Config::inject_file_env_for_runtime`
# copies it out of ~/.codescribe/.env into the process environment. An operator
# running their daily dictation on `phase1` therefore armed Layer 1 *inside this
# Layer-0 target* — and Layer 1 is supposed to diverge from Apple, so the bar
# went red for doing its job. Measured 2026-08-08, one binary, consecutive runs:
# `Other: 1` → 0.931 PASS, then `Other: 22` → 0.833 FAIL. Explicit `off` also
# blocks the injection at source (it only fills keys absent from the env).

# The lane the CALLER asked for — and it is exactly what the recipe pins below
# throw away. `make` cannot see a recipe-level `VAR=x cmd` assignment, but it
# does see `VAR=x make …` / `make VAR=x`, so `origin` separates "the caller
# pinned a lane" from "nobody asked". The operator's own `~/.codescribe/.env` is
# injected in-process by the core, never into this shell, so a daily `phase1`
# dotenv leaves this empty and the guard below stays silent for daily runs.
ifneq ($(origin CODESCRIBE_LAYERED_TRANSCRIPTION),undefined)
PARITY_LANE_REQUEST := $(CODESCRIBE_LAYERED_TRANSCRIPTION)
endif

# Refuse a run whose pin contradicts the request, instead of measuring the other
# lane and reporting it as the caller's.
#
# This is review finding P1-01 made impossible. A dispatch verifier ran
# `CODESCRIBE_LAYERED_TRANSCRIPTION=phase1 make test-engine-parity`; the recipe
# pin silently won, Layer 0 was measured twice, and the layered arm was recorded
# green while asserting nothing about Layer 1. The Rust-side guard
# (`measured_lane_matches_request`, tests/e2e_overlay_delivery_parity.rs) cannot
# catch that shape: by the time the test runs, request and measurement agree —
# they agree on the WRONG lane, because the intent was dropped one layer up,
# here. $(1) = lane this target pins, $(2) = target that actually measures the
# requested one.
define parity_lane_refuse
	@if [ -n "$(PARITY_LANE_REQUEST)" ] && [ "$(PARITY_LANE_REQUEST)" != "$(1)" ]; then \
	  printf 'parity lane refused: you asked for CODESCRIBE_LAYERED_TRANSCRIPTION=%s, but `%s` pins the lane to %s.\n' \
	    '$(PARITY_LANE_REQUEST)' '$@' '$(1)' >&2; \
	  printf 'The pin wins inside the recipe, so this run would measure %s and report that number as yours.\n' '$(1)' >&2; \
	  printf 'Measure what you asked for:  make %s\n' '$(2)' >&2; \
	  exit 2; \
	fi
endef

.PHONY: test-engine-parity
test-engine-parity: $(ENGINE_BRIDGE)
	$(call parity_lane_refuse,off,test-engine-parity-layered)
	@CODESCRIBE_LAYERED_TRANSCRIPTION=off \
	 CAPTURE_TEST=e2e_apple_live_parity \
	  ./scripts/e2e-blackhole-dictation.sh 05_apple-live-parity.wav

# Layer 1 armed: Apple live commits, then Whisper re-transcribes each sealed
# window and patches it in place (`ReplaceRange { source: TailPatch }`).
#
# NOT the same bar. This lane is judged on STRUCTURE ONLY — head present, tail
# sealed, no lost spans, measured lane matches the request. The Apple-fidelity
# numbers are printed and never asserted here, because Layer 1 exists to diverge
# from Apple toward what was actually said: gap-filling grows the denominator, so
# a MORE accurate layer scores LOWER against Apple. That is the mechanical shape
# of the layer working, not a contract breach.
#
# The single decisive fact behind the rule: `apple_reference_is_a_ruler_not_the_truth`
# pins the Apple reference at 0.805 against the human transcription of the same
# audio, so 1.000 on the Apple bar would mean reproducing Apple's ERRORS.
#
# Accuracy-vs-human is printed for both lanes and gates neither: its reference is
# a private fixture (`~/.codescribe/data_assets`), so a bar on it would evaporate
# on any tree without the operator's corpus. Which number gates a merge stays an
# operator decision — see
# .vibecrafted/plans/w12-layered-live-closure/reports/default-flip-memo-layered.md
#
# What Layer 1 must still not do is leave the number byte-identical: identical
# means the patches never reached the measured assembly (guarded always-on by
# `parity_assembly_reads_layer1_tail_patches`).
#
# Run both arms with `test-engine-parity-both`. SFSpeech is nondeterministic at
# word level (Layer-0 sample n=10 straddles 0.90: 0.778 … 0.931), so a single
# pair of runs is an observation, not a verdict.
.PHONY: test-engine-parity-layered
test-engine-parity-layered: $(ENGINE_BRIDGE)
	$(call parity_lane_refuse,phase1,test-engine-parity)
	@CODESCRIBE_LAYERED_TRANSCRIPTION=phase1 \
	 CAPTURE_TEST=e2e_apple_live_parity \
	  ./scripts/e2e-blackhole-dictation.sh 05_apple-live-parity.wav

# Both arms in one command, because "run both and compare" was a comment nobody
# could execute: until this target, `test-engine-parity-layered` was referenced
# by nothing at all — not a verifier, not a doc, not another target — so the
# layered arm existed on paper while every gate measured Layer 0 (review P1-01).
#
# This runs each arm through its own guarded target, retains both logs, and
# prints the two similarity numbers side by side with their delta. It does NOT
# invent a bar for the layered arm: the Layer-0 bar (0.90 vs the Apple-fidelity
# reference) is the wrong instrument for a layer whose job is to DIVERGE from
# Apple, and restating bars is an operator decision (see the memo path above).
# What this target owes you is two honest measurements from one command; the
# verdict on the delta stays human.
#
# Read the arms asymmetrically, because they are judged asymmetrically: arm 1
# (layer0) can go red on the similarity bar, arm 2 (layer1) can only go red on
# structure. An `rc=1` on arm 2 is therefore never "the number dropped".
#
# SFSpeech is nondeterministic at word level (measured spread 0.898–0.931 over
# 5 runs), so one pair of runs is an observation, not a verdict — run it a few
# times before believing a delta.
#
# `sim` reads the number from two places on purpose: the harness only echoes its
# `parity similarity` line on the PASS path, so a RED arm would otherwise report
# "<no measurement>" while its number sat in the assertion message. A red arm
# with a number is the whole point here — measured 2026-08-08, phase1 scored
# 0.863 against the Apple reference and the first version of this target hid it.
#
# Do NOT probe this target with `make -n`: the recipe recurses through $(MAKE),
# and GNU make runs such lines even in dry-run mode, so the tee'd arm logs get
# clobbered with dry-run output. The authoritative evidence is the per-run log
# the harness retains under target/e2e-blackhole/<fixture>-<timestamp>.log.
.PHONY: test-engine-parity-both
test-engine-parity-both:
	@if [ -n "$(PARITY_LANE_REQUEST)" ]; then \
	  printf 'parity lane refused: this target runs BOTH lanes, so pinning CODESCRIBE_LAYERED_TRANSCRIPTION=%s cannot be honoured.\n' \
	    '$(PARITY_LANE_REQUEST)' >&2; \
	  printf 'Drop the pin (`make test-engine-parity-both`), or measure one lane with test-engine-parity / test-engine-parity-layered.\n' >&2; \
	  exit 2; \
	fi
	@set -o pipefail; \
	 mkdir -p target/e2e-blackhole; \
	 off_log=target/e2e-blackhole/two-arm-layer0.log; \
	 on_log=target/e2e-blackhole/two-arm-phase1.log; \
	 sim() { \
	   v="$$(awk '/^parity similarity [0-9]/ { for (i = 1; i <= NF; i++) if ($$i == "=") v = $$(i + 1) } END { print v }' "$$1")"; \
	   [ -n "$$v" ] || v="$$(awk -F'similarity ' '/similarity [0-9.]+ < bar/ { split($$2, a, " "); v = a[1] } END { print v }' "$$1")"; \
	   printf '%s' "$$v"; \
	 }; \
	 acc() { \
	   awk '/^parity accuracy-vs-human [0-9]/ { for (i = 1; i <= NF; i++) if ($$i == "=") v = $$(i + 1) } END { print v }' "$$1"; \
	 }; \
	 echo "=== arm 1/2 — Layer 0 (lane pinned off) ==="; \
	 $(MAKE) --no-print-directory test-engine-parity 2>&1 | tee "$$off_log"; \
	 off_rc=$$?; \
	 echo "=== arm 2/2 — Layer 1 (lane pinned phase1) ==="; \
	 $(MAKE) --no-print-directory test-engine-parity-layered 2>&1 | tee "$$on_log"; \
	 on_rc=$$?; \
	 off_sim="$$(sim "$$off_log")"; on_sim="$$(sim "$$on_log")"; \
	 off_acc="$$(acc "$$off_log")"; on_acc="$$(acc "$$on_log")"; \
	 echo "=== two-arm parity ==="; \
	 printf 'layer0 (off)    similarity %s  accuracy-vs-human %s  (rc=%s, log %s)\n' \
	   "$${off_sim:-<no measurement>}" "$${off_acc:-<none>}" "$$off_rc" "$$off_log"; \
	 printf 'layer1 (phase1) similarity %s  accuracy-vs-human %s  (rc=%s, log %s)\n' \
	   "$${on_sim:-<no measurement>}" "$${on_acc:-<none>}" "$$on_rc" "$$on_log"; \
	 if [ -n "$$off_sim" ] && [ -n "$$on_sim" ]; then \
	   awk -v a="$$off_sim" -v b="$$on_sim" 'BEGIN { printf "delta similarity (phase1 - off) %+.3f  <- fidelity to APPLE; gap-fill lowers this by design\n", b - a }'; \
	   if [ "$$off_sim" = "$$on_sim" ]; then \
	     printf 'WARNING: arms are byte-identical — either the tail patches never reached the measured assembly, or the clip has nothing to patch. See parity_assembly_reads_layer1_tail_patches.\n' >&2; \
	   fi; \
	   if [ -n "$$off_acc" ] && [ -n "$$on_acc" ]; then \
	     awk -v a="$$off_acc" -v b="$$on_acc" 'BEGIN { printf "delta accuracy  (phase1 - off) %+.3f  <- fidelity to what was SAID; this is the sign that answers \"is Layer 1 better?\"\n", b - a }'; \
	   else \
	     printf 'delta accuracy: <none> — no human transcription beside the fixture, so neither arm was scored on accuracy. The similarity delta alone CANNOT tell you whether Layer 1 is better; it only tells you it is different.\n' >&2; \
	   fi; \
	 else \
	   printf 'no delta: an arm printed no similarity number. Since the harness now surfaces its measurements on the FAILING path too, this means the arm stopped before scoring at all — a precondition failure (missing loopback, no mic grant, no fixture) rather than a parity verdict. Read the arm log; do NOT report this as a red bar.\n' >&2; \
	 fi; \
	 [ "$$off_rc" -eq 0 ] && [ "$$on_rc" -eq 0 ]

# Host smoke for the macOS surfaces we own — run after every OS/Xcode bump.
# Headless, raises no TCC dialog, posts no synthetic events; operator-only rows
# report SKIP instead of passing quietly. SMOKE_ARGS='--with-inference' adds the
# Metal/candle cold-vs-warm row. See docs in scripts/smoke-macos27.sh.
.PHONY: smoke-macos27
smoke-macos27:
	@./scripts/smoke-macos27.sh $(SMOKE_ARGS)

# SwiftUI front-end unit tests (macos/CodescribeTests) — the only gate that
# executes them. 318 tests in ~4.3 s once the dylib and project exist.
#
# Deliberately NOT wired into `check`: it needs Xcode and a built ffi dylib,
# and the self-hosted CI runners are cargo-only. Run it locally after touching
# anything under macos/.
#
# Two invocation traps this target exists to close, both of which read as
# "the Swift tests cannot run here" when hit by hand:
#
#   1. `CODE_SIGN_IDENTITY="-" xcodebuild …` does nothing. xcodebuild takes
#      build settings from ARGUMENTS, never from the environment, so the
#      env-prefix form leaves project.yml's `Codescribe Dev` identity in place
#      and the build dies with "No certificate matching 'Codescribe Dev'
#      found" — before a single test runs. It must be a positional KEY=value.
#   2. `-scheme Codescribe` is mandatory. xcodegen emits no scheme for
#      bundle.unit-test targets, so no `-target CodescribeTests` invocation can
#      resolve the SPM dependencies.
#
# The identity is ad-hoc on purpose and is NOT $(CODESCRIBE_CODESIGN_IDENTITY):
# handing xcodebuild a real "Apple Development: …" identity propagates to the
# SPM package targets, which then fail with `Signing for
# "HighlightSwift_HighlightSwift" requires a development team`. Tests need a
# host that launches, not a distributable identity.
#
# Narrow a run with SWIFT_TEST_ARGS:
#   make test-swift SWIFT_TEST_ARGS='-only-testing:CodescribeTests/OverlayStateTests'
#
# WALL-CLOCK BUDGET. This gate used to report rc=0 whether the suite took 4 s or
# 47 s, which is how a 10x regression lived in the tree with "4.1-4.5 s (n=4)"
# recorded beside it as fact. Measured 2026-08-08, identical tree, identical 317
# tests, three consecutive runs: 47.5 / 28.2 / 4.5 s at ~3 s of CPU — the spread
# was BLOCKING (real Keychain calls from an XCTest host the core read as
# production), not work. Fixed at the source in core/config/keychain.rs; the
# budget stays because the next such regression should be a red gate, not
# folklore about an unexplained hang.
#
# 30 s is ~6x the measured fast mode and would have failed both bad runs above,
# while leaving room for a loaded host (this machine also runs CI). Raise it for
# a genuinely busy box rather than deleting it:
#   make test-swift SWIFT_TEST_MAX_SECONDS=90
SWIFT_TEST_CODESIGN_IDENTITY ?= -
SWIFT_TEST_MAX_SECONDS ?= 30
.PHONY: test-swift
test-swift: $(ENGINE_BRIDGE)
	@set -o pipefail; \
	$(TEST_DATA_DIR_SETUP); \
	echo "=== Apple phrase-restart Rust/Swift lockstep self-test ==="; \
	$(ENGINE_BRIDGE) --phrase-restart-self-test || exit $$?; \
	if [ ! -f target/$(PROFILE)/libcodescribe_ffi.dylib ]; then \
	  echo "test-swift: target/$(PROFILE)/libcodescribe_ffi.dylib is missing." >&2; \
	  echo "test-swift: run 'make app-bindings' (or 'make app') first." >&2; \
	  exit 2; \
	fi; \
	echo "=== Swift front-end tests (CodescribeTests) ==="; \
	cd macos && xcodebuild test \
	  -scheme Codescribe \
	  -destination 'platform=macOS,arch=arm64' \
	  CODE_SIGN_IDENTITY="$(SWIFT_TEST_CODESIGN_IDENTITY)" \
	  $(SWIFT_TEST_ARGS) 2>&1 | tee $(SWIFT_TEST_LOG) | \
	  grep -E "^Test Case .* (failed|error)|Executed [0-9]+ tests|^\*\* TEST|error:"; \
	rc=$${PIPESTATUS[0]}; \
	executed=$$(grep -oE 'Executed [0-9]+ test' $(SWIFT_TEST_LOG) | tail -1 | grep -oE '[0-9]+'); \
	if [ "$$rc" -eq 0 ] && [ "$${executed:-0}" -eq 0 ]; then \
	  echo "test-swift: xcodebuild said TEST SUCCEEDED but executed 0 tests." >&2; \
	  echo "test-swift: a -only-testing filter that matches nothing exits 0 — that is a" >&2; \
	  echo "test-swift: silent pass, not a green gate. Check SWIFT_TEST_ARGS." >&2; \
	  rc=3; \
	fi; \
	secs=$$(sed -nE 's/^.*Executed [0-9]+ tests?,.* in ([0-9.]+) \([0-9.]+\) seconds.*$$/\1/p' $(SWIFT_TEST_LOG) | tail -1); \
	slowest=$$(sed -nE "s/^.*CodescribeTests\.([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' passed \(([0-9.]+) seconds\)\..*$$/\3 \1.\2/p" $(SWIFT_TEST_LOG) | sort -rn | head -1); \
	echo "test-swift: full log $(SWIFT_TEST_LOG) (rc=$$rc, executed=$${executed:-0}, seconds=$${secs:-unknown})"; \
	if [ -n "$$slowest" ]; then echo "test-swift: slowest test $$slowest"; fi; \
	if [ "$$rc" -eq 0 ] && [ -n "$$secs" ] && \
	   awk -v s="$$secs" -v m="$(SWIFT_TEST_MAX_SECONDS)" 'BEGIN{exit !(s>m)}'; then \
	  echo "test-swift: suite took $$secs s, over the $(SWIFT_TEST_MAX_SECONDS) s budget." >&2; \
	  echo "test-swift: green-but-slow is the shape this gate exists to catch — a 10x swing" >&2; \
	  echo "test-swift: here has meant the core is doing real (blocking) work for a test run," >&2; \
	  echo "test-swift: not that the machine is busy. Check the slowest test above, then" >&2; \
	  echo "test-swift: core/config/keychain.rs::in_xctest_host and macos/CodescribeTests/README.md." >&2; \
	  echo "test-swift: if the host really is loaded: make test-swift SWIFT_TEST_MAX_SECONDS=90" >&2; \
	  rc=4; \
	fi; \
	exit $$rc

# Apple live engine proof.
#
# SEPARATION: daily Codescribe (mic + speakers + teacher) is independent of
# this target. ENGINE_ALL_CLIPS=1 uses BlackHole ONLY inside the harness
# (play/capture by device name); system Sound defaults are snapshotted and
# restored so daily input/output is never left on BlackHole.
#
# Without ENGINE_ALL_CLIPS: channel path (WAV→session) — no BlackHole, fine
# while using the app for real dictation on the same machine.
#
# With ENGINE_ALL_CLIPS=1: player → BlackHole → cpal for each 01–04 clip, then
# the parity bar. Requires brew blackhole-2ch + terminal Microphone TCC +
# `make engine-auth` (bridge Speech). Do NOT set BH as system default.
test-engine-apple: $(ENGINE_BRIDGE)
	@if [ "$(ENGINE_ALL_CLIPS)" = "1" ]; then \
	  set -e; \
	  echo "=== harness path (BlackHole isolated; daily Sound defaults restored on exit) ==="; \
	  for clip in $(DATA_ASSETS_DIR)/0[1-4]_*.wav; do \
	    echo "=== BlackHole device capture: $$clip ==="; \
	    ./scripts/e2e-blackhole-dictation.sh "$$clip"; \
	  done; \
	  $(MAKE) test-engine-parity; \
	else \
	  echo "=== channel path (no BlackHole; safe alongside daily Codescribe) ==="; \
	  $(MAKE) test-engine-apple-channel; \
	fi

# Channel-injection mic-sim (fast, no loopback device): Apple live multi-seal
# freezed+append + Whisper file final. Proves decoder+pipeline, NOT capture.
# Verbose live log: RUST_LOG + line-buffered tee (see ENGINE_RUST_LOG / ENGINE_CARGO_TEST_LIVE).
.PHONY: test-engine-apple-channel
test-engine-apple-channel: $(ENGINE_BRIDGE)
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== Core engine Apple live (multi-utterance freezed+append) ===" | tee -a "$$LOG"; \
	echo "  clip=$(ENGINE_CLIP)  all_clips=$(ENGINE_ALL_CLIPS)" | tee -a "$$LOG"; \
	echo "  bridge=$(CURDIR)/$(ENGINE_BRIDGE)" | tee -a "$$LOG"; \
	echo "  RUST_LOG=$(ENGINE_RUST_LOG)  (override: ENGINE_RUST_LOG=warn make …)" | tee -a "$$LOG"; \
	echo "  note: ~1–3 min/clip of Apple STT; heartbeats + tracing stream live" | tee -a "$$LOG"; \
	$(ENV_LOAD); \
	export CODESCRIBE_STT_ENGINE=apple; \
	export CODESCRIBE_APPLE_STT_BRIDGE="$(CURDIR)/$(ENGINE_BRIDGE)"; \
	export CODESCRIBE_BRIDGE_DISCLAIM=1; \
	export CODESCRIBE_E2E_STT=1; \
	export RUST_LOG="$(ENGINE_RUST_LOG)"; \
	export RUST_LOG_STYLE=always; \
	if [[ "$(ENGINE_ALL_CLIPS)" == "1" ]]; then \
	  export CODESCRIBE_E2E_ALL_CLIPS=1; \
	  unset CODESCRIBE_E2E_AUDIO || true; \
	else \
	  export CODESCRIBE_E2E_AUDIO="$(ENGINE_CLIP)"; \
	fi; \
	$(ENGINE_CARGO_TEST_LIVE); \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

# Same engine bar on Candle live (no Apple; useful CI / offline).
test-engine-candle:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== Core engine Candle live (multi-utterance freezed+append) ===" | tee -a "$$LOG"; \
	echo "  RUST_LOG=$(ENGINE_RUST_LOG)" | tee -a "$$LOG"; \
	$(ENV_LOAD); \
	export CODESCRIBE_STT_ENGINE=candle; \
	export CODESCRIBE_E2E_STT=1; \
	export RUST_LOG="$(ENGINE_RUST_LOG)"; \
	export RUST_LOG_STYLE=always; \
	if [[ "$(ENGINE_ALL_CLIPS)" == "1" ]]; then \
	  export CODESCRIBE_E2E_ALL_CLIPS=1; \
	  unset CODESCRIBE_E2E_AUDIO || true; \
	else \
	  export CODESCRIBE_E2E_AUDIO="$(ENGINE_CLIP)"; \
	fi; \
	$(ENGINE_CARGO_TEST_LIVE); \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "Done. Log: $$LOG" | tee -a "$$LOG"

# Teacher CLI: live×whisper×human → Needs attention → lexicon (built-in proof fixture).
test-teacher:
	@echo "=== Teacher proof (built-in e2e 01 texts; no mic) ==="
	@cargo run --bin codescribe-teacher -- proof --html /tmp/codescribe-teacher.html
	@echo "HTML: /tmp/codescribe-teacher.html  (open /tmp/codescribe-teacher.html)"

test-all:
	@$(TEST_SETUP); \
	set -o pipefail; \
	echo "=== Full Test Suite ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "=== Ignored / Real API ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test --workspace --all-targets -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "=== Full Pipeline (STT) ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); CODESCRIBE_E2E_STT=1 \
	cargo test --test e2e_full_pipeline -- --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
	echo "=== SSE Streaming ===" | tee -a "$$LOG"; \
	$(ENV_LOAD); $(APPLY_TEST_LLM); \
	cargo test e2e_sse --release -- --ignored --nocapture 2>&1 | tee -a "$$LOG"; test_rc=$${PIPESTATUS[0]}; \
	if [[ $$test_rc -ne 0 ]]; then exit $$test_rc; fi; \
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

# Static gate: everything that can be decided without running the product.
#
# This target used to end with a bare "Quality gate passed" while executing not
# one test, and .github/workflows/rust.yml called it "the full local gate (incl.
# real-API / heavy e2e tests)". Both readings were wrong in the same direction —
# toward more confidence than the commands earn. The closing line now names what
# ran and what did not; `make verify` is the target that runs tests.
check:
	@echo "=== Format Check (Rust) ==="
	@cargo fmt --all -- --check
	@echo "=== Format Check (non-Rust) ==="
	@npx --yes prettier@2.7.1 --check . --ignore-path .prettierignore --ignore-unknown
	@echo "=== Clippy (workspace, all targets) ==="
	@cargo clippy --workspace --all-targets -- -D warnings
	@echo "=== Semgrep ==="
	@semgrep scan --config auto --error .
	@echo "=== Env registry ==="
	@bash scripts/validate-envs.sh
	@echo "=== Gate ledger ==="
	@bash scripts/validate-gates.sh
	@echo ""
	@echo "check: static gate passed — format, lint, security, env registry, gate ledger."
	@echo "check: NO tests were executed. Run 'make verify' for the test gate."

# The hermetic gate — and the one CI runs, by name (.github/workflows/rust.yml).
#
# Hermetic means: nothing here reaches for this host. It does NOT source
# ~/.codescribe/.env the way every `make test*` target does via ENV_LOAD, it
# opens no Console window, it needs no API key, no audio device, no Xcode and no
# private fixture corpus. That is the whole reason it can be both the agent's
# gate and CI's job: one command, one definition site, no drift between them.
#
# CODESCRIBE_DISABLE_KEYCHAIN=1 keeps keychain-backed tests off the real macOS
# Keychain (no interactive unlock on a runner — see core/config/keychain.rs);
# CODESCRIBE_NO_EMBED=1 is build-time and keeps the model payload out, so the
# gate builds the same way on a runner as it does for an agent on this host.
#
# --all-targets does NOT include doctests, so they are run explicitly after it;
# dropping that second line would silently narrow the gate.
#
# `set -e` is load-bearing, not boilerplate. A `;`-joined recipe takes the exit
# code of its LAST command, so without it this target printed "hermetic gate
# passed" and returned 0 over `error: test failed, to rerun pass -p
# codescribe-ffi --lib` — the first run of this gate produced exactly the lie it
# was written to remove. Any line added below must stay in the `-e` chain.
verify:
	@set -eo pipefail; \
	$(TEST_DATA_DIR_SETUP); \
	echo "=== Verify (hermetic: workspace tests) ==="; \
	CODESCRIBE_NO_EMBED=1 CODESCRIBE_DISABLE_KEYCHAIN=1 \
	  cargo test --workspace --all-targets; \
	echo "=== Verify (hermetic: doctests) ==="; \
	CODESCRIBE_NO_EMBED=1 CODESCRIBE_DISABLE_KEYCHAIN=1 \
	  cargo test --workspace --doc; \
	echo "=== Verify (env registry) ==="; \
	bash scripts/validate-envs.sh; \
	echo "=== Verify (gate ledger) ==="; \
	bash scripts/validate-gates.sh; \
	echo ""; \
	echo "verify: hermetic gate passed."; \
	echo "verify: NOT covered here — every class=operator target in the GATE LEDGER"; \
	echo "verify: (parity bars, Swift front-end suite, host smoke, real-API e2e)."

# Print the classified verification surface. Asserts nothing, so it carries no
# ledger row of its own — it only shows the ledger `check` and `verify` enforce.
.PHONY: gate-ledger
gate-ledger:
	@bash scripts/validate-gates.sh --list

# ── Canaries: claims vs. execution truth ─────────────────────────────────────
# The env registry says which vars EXIST; the gate ledger says what gates RUN.
# Neither checks whether a VALUE the repo claims is still the value the code
# executes — the gap every recent expensive surprise lived in (docs said the
# idle-unload default was 300 s while the code ran 2700 s; a benchmark measured
# a hard-coded model path; a release died on its LAST gate over a key with no
# local source). scripts/canaries.sh is the catalog: every row names the
# incident it was born from. `--list` prints the catalog without running it.
verify-canaries:
	@bash scripts/canaries.sh

smoke-canaries:
	@bash scripts/canaries.sh --host

.PHONY: canary-catalog
canary-catalog:
	@bash scripts/canaries.sh --list

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
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release' 'Build release dylib slim (Silero + MiniLM; Whisper runtime)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install' 'Install CLI slim (Whisper via cache/Settings, not embedded)'
	@printf '%s\n' '  make install-no-embed DEV/RECOVERY: no optional embeds (runtime paths only)'
	@printf '%s\n' '  make release-codescribe-embedded Fat dylib with Whisper baked in (not daily)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'config' 'Edit ~/.codescribe/.env'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'install-app' 'Install to /Applications'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'RELEASE & DISTRIBUTION'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'dmg' 'Build DMG (ad-hoc signed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'dmg-signed' 'Build DMG (Developer ID signed)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-standard' 'Sign+notarize slim DMG (no Whisper embed — daily public)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-full' 'Sign+notarize fat _full DMG (Whisper embedded, optional)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'release-dmgs' 'Build slim + fat notarized DMGs'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'notarize' 'Notarize DMG with Apple'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'verify-dmg' 'Fail-closed payload gate (DMG=… VARIANT=slim|full VERSION=X.Y.Z)'
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
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'QUALITY — GATES (run anywhere, decide merge)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'check' 'Static gate: fmt + prettier + clippy + semgrep + registries. NO tests'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'verify' 'Hermetic test gate — exactly what CI runs (rust.yml)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'lint' 'Run clippy + fmt check'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'format' 'Format Rust code'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'fix' 'Format all code (Rust + Prettier)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'semgrep' 'Run release security scan'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'hooks' 'Install pre-commit + pre-push + commit-msg hooks'
	@printf '\n'
	@printf '  $(HELP_C_YELLOW)%s$(HELP_C_RESET)\n' 'QUALITY — BENCH INSTRUMENTS (this host only, never a merge gate)'
	@printf '%s\n' '  Full classification: make -s gate-ledger'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test' 'Full suite incl. ignored real-API tests (sources ~/.codescribe/.env)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-quick' 'Workspace tests, no real API (sources ~/.codescribe/.env)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-swift' 'SwiftUI suite + phrase-restart lockstep (needs Xcode + ffi dylib)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'smoke-macos27' 'Host smoke after an OS/Xcode bump (SMOKE_ARGS=--with-inference)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-e2e' 'Run E2E tests (mock)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-e2e-real' 'Run E2E tests with real API (needs LLM_*_API_KEY)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-sse' 'Run SSE streaming tests (real API)'
	@printf '%s\n' '  make test-formatting Run AI formatting tests'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine' 'Core freezed+append unit bar (fast, no STT)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine-apple' 'Apple live multi-utterance e2e (ENGINE_CLIP / ENGINE_ALL_CLIPS=1)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine-candle' 'Candle live multi-utterance e2e (same engine bar)'
	@printf '%s\n' '  make test-engine-parity-both Both parity arms + delta (needs the private corpus)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-teacher' 'Teacher CLI proof HTML (live×whisper×human)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-all' 'Run full test suite'

# ============================================================================
# Release & Distribution
# ============================================================================

# Distribution preflight — fail in a second, not two minutes into cargo.
# Every release-profile build needs the production licence key; core/build.rs
# panics without it, but only after compiling codescribe-core.
dist-preflight:
	@key='$(CODESCRIBE_DIST_LICENSE_KEY)'; \
	if [ $${#key} -ne 64 ] || [ -n "$$(printf '%s' "$$key" | tr -d '0-9a-fA-F')" ]; then \
		echo "ERROR: distribution builds need the production licence verification key (64 hex chars)."; \
		echo "  looked in: \$$CODESCRIBE_LICENSE_PUBLIC_KEY_HEX, then $(CODESCRIBE_LICENSE_PUBLIC_KEY_FILE)"; \
		echo "  fix either:"; \
		echo "    printf %s <64-hex> > $(CODESCRIBE_LICENSE_PUBLIC_KEY_FILE)"; \
		echo "    export CODESCRIBE_LICENSE_PUBLIC_KEY_HEX=<64-hex>"; \
		echo "  note: 'VAR=x make a && make b' scopes VAR to 'make a' only — use export,"; \
		echo "        or the second command builds without the key and dies inside build.rs."; \
		exit 1; \
	fi
	@echo "dist preflight: licence key OK (64 hex from $(if $(CODESCRIBE_LICENSE_PUBLIC_KEY_HEX),environment,$(CODESCRIBE_LICENSE_PUBLIC_KEY_FILE)))"

# Signed artifacts additionally need a Developer ID (not the Apple Development
# identity `make install-app` prefers for TCC stability).
dist-preflight-signed: dist-preflight
	@ident='$(CODESCRIBE_DIST_CODESIGN_IDENTITY)'; \
	if [ -z "$$ident" ] || [ "$$ident" = "-" ]; then \
		echo "ERROR: signed distribution needs a 'Developer ID Application' identity."; \
		echo "  check with: security find-identity -v -p codesigning"; \
		echo "  override with: make ... CODESCRIBE_DIST_CODESIGN_IDENTITY='Developer ID Application: ...'"; \
		exit 1; \
	fi
	@echo "dist preflight: signing identity OK ($(CODESCRIBE_DIST_CODESIGN_IDENTITY))"
	@sk='$(CODESCRIBE_DIST_SPARKLE_KEY)'; \
	if [ -z "$$sk" ] || [ "$$(printf '%s' "$$sk" | base64 -d 2>/dev/null | wc -c | tr -d ' ')" != "32" ]; then \
		echo "ERROR: signed distribution needs the Sparkle update public key (Ed25519, base64)."; \
		echo "  looked in: \$$SPARKLE_ED_PUBLIC_KEY, then $(CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE)"; \
		echo "  without it the bundle ships an empty SUPublicEDKey and"; \
		echo "  scripts/verify-dmg-payload.sh refuses the DMG *after* notarisation —"; \
		echo "  the most expensive place in the pipeline to discover a missing input."; \
		exit 1; \
	fi
	@echo "dist preflight: Sparkle public key OK (32-byte Ed25519 from $(if $(SPARKLE_ED_PUBLIC_KEY),environment,$(CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE)))"

# Daily slim DMG (public default): Silero + MiniLM, Whisper NOT embedded.
dmg: dist-preflight
	@CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" ./scripts/build-dmg.sh

dmg-signed: dist-preflight-signed
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" \
	 CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 SPARKLE_ED_PUBLIC_KEY="$(CODESCRIBE_DIST_SPARKLE_KEY)" \
	 ./scripts/build-dmg.sh --sign

# Daily signed+notarized public artifact (same as make dmg-signed + notarize).
# Does NOT download/embed Whisper. Apple STT works out of the box; Whisper is
# opt-in via Settings → Dictation download (or make download-model).
# Ends with the fail-closed payload gate (signed ≠ complete; see 0.13.2 MiniLM miss).
release-standard: dist-preflight-signed
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" \
	 CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 SPARKLE_ED_PUBLIC_KEY="$(CODESCRIBE_DIST_SPARKLE_KEY)" \
	 ./scripts/build-dmg.sh --sign --notarize
	@VERSION=$$(awk -F '"' '/^version[[:space:]]*=/{print $$2; exit}' Cargo.toml); \
	HEAD_SHA=$$(git rev-parse --short=9 HEAD 2>/dev/null || echo nogit); \
	DMG=$$(ls -t Codescribe_$${VERSION}-*-$${HEAD_SHA}.dmg 2>/dev/null | head -1); \
	if [ -z "$$DMG" ]; then \
		echo "ERROR: no slim DMG for HEAD $$HEAD_SHA / version $$VERSION after build"; \
		exit 1; \
	fi; \
	./scripts/verify-dmg-payload.sh "$$DMG" --variant slim --version "$$VERSION"

# Optional fat SKU: bake Whisper (~1GB+) into the app. Not the daily path.
# Ends with the fail-closed payload gate (full = Silero + MiniLM + Whisper).
release-full: dist-preflight-signed ensure-models
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" \
	 CODESCRIBE_LICENSE_PUBLIC_KEY_HEX="$(CODESCRIBE_DIST_LICENSE_KEY)" \
	 SPARKLE_ED_PUBLIC_KEY="$(CODESCRIBE_DIST_SPARKLE_KEY)" \
	 ./scripts/build-dmg.sh --sign --notarize --embed-whisper --dmg-suffix _full
	@VERSION=$$(awk -F '"' '/^version[[:space:]]*=/{print $$2; exit}' Cargo.toml); \
	HEAD_SHA=$$(git rev-parse --short=9 HEAD 2>/dev/null || echo nogit); \
	DMG=$$(ls -t Codescribe_$${VERSION}-*-$${HEAD_SHA}_full.dmg 2>/dev/null | head -1); \
	if [ -z "$$DMG" ]; then \
		echo "ERROR: no full DMG for HEAD $$HEAD_SHA / version $$VERSION after build"; \
		exit 1; \
	fi; \
	./scripts/verify-dmg-payload.sh "$$DMG" --variant full --version "$$VERSION"

# Both public variants: slim first, then fat _full.
release-dmgs: release-standard release-full

# Fail-closed payload gate (manual / CI). Usage:
#   make verify-dmg DMG=path VARIANT=slim|full VERSION=X.Y.Z [SKIP_NOTARY=1]
verify-dmg:
	@if [ -z "$(DMG)" ] || [ -z "$(VARIANT)" ] || [ -z "$(VERSION)" ]; then \
		echo "Usage: make verify-dmg DMG=path VARIANT=slim|full VERSION=X.Y.Z [SKIP_NOTARY=1]"; \
		exit 2; \
	fi
	@./scripts/verify-dmg-payload.sh "$(DMG)" --variant "$(VARIANT)" --version "$(VERSION)" $(if $(filter 1,$(SKIP_NOTARY)),--skip-notary,)

notarize:
	@if ls Codescribe_*.dmg 1> /dev/null 2>&1; then \
		DMG=$$(ls -t Codescribe_*.dmg | head -1); \
		HEAD_SHA=$$(git rev-parse --short=9 HEAD 2>/dev/null || echo nogit); \
		case "$$DMG" in \
		*"$$HEAD_SHA"*) ./scripts/notarize.sh "$$DMG";; \
		*) if [ "$(FORCE_NOTARIZE)" = "1" ]; then \
			echo "FORCE_NOTARIZE=1 — notarizing '$$DMG' despite HEAD mismatch ($$HEAD_SHA)"; \
			./scripts/notarize.sh "$$DMG"; \
		else \
			echo "REFUSING: newest DMG '$$DMG' was not cut from HEAD ($$HEAD_SHA)."; \
			echo "Stapling a stale artifact is how you end up testing a two-day-old build."; \
			echo "Run 'make dmg-signed' first (it names the DMG after the commit it was cut"; \
			echo "from), or override consciously with FORCE_NOTARIZE=1."; \
			exit 1; \
		fi;; \
		esac; \
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
