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
# Distribution artifacts (signed DMG + notarization) require Developer ID, not
# the TCC-friendly Apple Development identity preferred above for local
# installs. Recipes must pass this into build-dmg.sh explicitly — make vars are
# not exported to recipe children, which is exactly how `make release-standard`
# used to reach --sign with an empty identity.
CODESCRIBE_DIST_CODESIGN_IDENTITY ?= $(if $(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),$(strip $(CODESCRIBE_DEVELOPER_ID_IDENTITY)),$(CODESCRIBE_CODESIGN_IDENTITY))
CODESCRIBE_APP_NAME ?= Codescribe
CODESCRIBE_DISPLAY_NAME ?= Codescribe
# SwiftUI app build profile for `make app` / `make app-bindings` (debug|release)
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
release-codescribe:
	@echo "Building codescribe-ffi (release dylib, embedded: Silero + MiniLM; Whisper runtime)..."
	@echo "  The app front-end is no longer a Rust bin; this builds the UniFFI bridge dylib."
	@echo "  Produce the runnable SwiftUI app with: make app PROFILE=release"
	@echo "  Fat Whisper embed: make release-codescribe-embedded"
	@env -u CODESCRIBE_EMBED_WHISPER -u CODESCRIBE_NO_EMBED cargo build --release -p codescribe-ffi

# Optional fat SKU / offline curiosity: bake Whisper into the dylib (~1GB+).
# Not the daily release path. Pair with `make release-full` for a _full DMG.
release-codescribe-embedded: ensure-models
	@echo "Building codescribe-ffi (FAT: Silero + MiniLM + Whisper embedded)..."
	@CODESCRIBE_EMBED_WHISPER=1 cargo build --release -p codescribe-ffi

# ── SwiftUI app (macos/) via the codescribe-ffi UniFFI bridge ────────────────
# Full verified pipeline: cargo (ffi dylib) → uniffi-bindgen → xcodegen → xcodebuild.
# `app-bindings` stops after xcodegen (no Xcode needed) for fast Rust-side iteration.
app:
	@echo "Building Codescribe.app (SwiftUI, PROFILE=$(PROFILE))..."
	@./scripts/build-app.sh $(PROFILE)

app-bindings:
	@echo "Regenerating UniFFI Swift bindings + Xcode project (PROFILE=$(PROFILE), no xcodebuild)..."
	@SKIP_XCODEBUILD=1 ./scripts/build-app.sh $(PROFILE)

release-qube:
	@echo "Building qube-* (release, runtime model resolve from HF cache)..."
	@CODESCRIBE_NO_EMBED=1 cargo build --release --target-dir target-noembed --bin qube-daemon --bin qube-report

release: release-codescribe release-qube

install:
	@echo "Installing qube tools (slim: Silero + MiniLM; Whisper from cache / Settings)..."
	@./scripts/download-embedder.sh || true
	@env -u CODESCRIBE_EMBED_WHISPER -u CODESCRIBE_NO_EMBED cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
	@$(MAKE) hooks
	@echo "Installed: qube tools $$(grep '^version' $(VERSION_FILE) | head -1 | sed 's/.*\"\(.*\)\"/v\1/')"
	@echo "Note: Whisper is not embedded — download via Settings → Dictation or make download-model"

install-no-embed:
	@echo "Installing qube tools (DEV/RECOVERY: no optional embeds; runtime paths only)..."
	@CODESCRIBE_NO_EMBED=1 cargo install --path . --force
	@mkdir -p ~/.codescribe
	@pwd > ~/.codescribe/repo_path
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
	@echo "Building $(CODESCRIBE_APP_NAME).app (SwiftUI, release) via scripts/build-app.sh ..."
	@$(MAKE) --no-print-directory app PROFILE=release
	@APP_SRC="macos/build/Build/Products/Release/Codescribe.app"; \
	if [ ! -d "$$APP_SRC" ]; then \
		echo "Build product missing: $$APP_SRC — 'make app PROFILE=release' did not produce the app."; \
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
# Clip for STT engine e2e (mic-sim → transcription_session). Override:
#   make test-engine-apple ENGINE_CLIP=tests/assets/data_assets/01_no-to-dobra.wav
#   make test-engine-apple ENGINE_ALL_CLIPS=1
ENGINE_CLIP ?= tests/assets/data_assets/02_kubernetes-wymaga-konfiguracji.wav
ENGINE_ALL_CLIPS ?= 0
ENGINE_BRIDGE ?= target/release/codescribe-stt-bridge
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
	@echo "Building codescribe-stt-bridge → $(ENGINE_BRIDGE)"
	@mkdir -p $(dir $(ENGINE_BRIDGE))
	@swiftc -O -o $(ENGINE_BRIDGE) core/stt/apple_stt/codescribe-stt-bridge.swift \
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
.PHONY: test-engine-parity
test-engine-parity: $(ENGINE_BRIDGE)
	@CAPTURE_TEST=e2e_apple_live_parity \
	  ./scripts/e2e-blackhole-dictation.sh tests/assets/data_assets/05_apple-live-parity.wav

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
	  for clip in tests/assets/data_assets/0[1-4]_*.wav; do \
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

check:
	@echo "=== Format Check (Rust) ==="
	@cargo fmt --all -- --check
	@echo "=== Format Check (non-Rust) ==="
	@npx --yes prettier@2.7.1 --check . --ignore-path .prettierignore --ignore-unknown
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
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine' 'Core freezed+append unit bar (fast, no STT)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine-apple' 'Apple live multi-utterance e2e (ENGINE_CLIP / ENGINE_ALL_CLIPS=1)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-engine-candle' 'Candle live multi-utterance e2e (same engine bar)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-teacher' 'Teacher CLI proof HTML (live×whisper×human)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'test-all' 'Run full test suite'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'check' 'Verify formatting + clippy + semgrep (CI-safe)'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'semgrep' 'Run release security scan'
	@printf '    $(HELP_C_GREEN)%-18s$(HELP_C_RESET) %s\n' 'hooks' 'Install pre-commit + pre-push + commit-msg hooks'

# ============================================================================
# Release & Distribution
# ============================================================================

# Daily slim DMG (public default): Silero + MiniLM, Whisper NOT embedded.
dmg:
	@./scripts/build-dmg.sh

dmg-signed:
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" ./scripts/build-dmg.sh --sign

# Daily signed+notarized public artifact (same as make dmg-signed + notarize).
# Does NOT download/embed Whisper. Apple STT works out of the box; Whisper is
# opt-in via Settings → Dictation download (or make download-model).
release-standard:
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" ./scripts/build-dmg.sh --sign --notarize

# Optional fat SKU: bake Whisper (~1GB+) into the app. Not the daily path.
release-full: ensure-models
	@CODESCRIBE_CODESIGN_IDENTITY="$(CODESCRIBE_DIST_CODESIGN_IDENTITY)" ./scripts/build-dmg.sh --sign --notarize --embed-whisper --dmg-suffix _full

# Both public variants: slim first, then fat _full.
release-dmgs: release-standard release-full

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
