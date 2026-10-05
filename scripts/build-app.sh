#!/usr/bin/env bash
#
# Build the Codescribe SwiftUI app from the Rust `codescribe-ffi` bridge.
#
# This is the single source of truth for the SwiftUI build pipeline. Before it
# existed the steps below lived only in tribal memory / a reviewer's shell
# history, and a clean checkout could not produce a runnable app (the generated
# UniFFI bindings, Info.plist and .xcodeproj are all gitignored).
#
# Pipeline (each stage is deterministic and rerunnable):
#   1. cargo build -p codescribe-ffi   -> libcodescribe_ffi.dylib + uniffi-bindgen
#   2. install_name_tool -id @rpath/... -> make the dylib relocatable
#   3. uniffi-bindgen generate          -> Swift bindings into macos/Codescribe/Bridge
#   4. xcodegen generate                -> macos/Codescribe.xcodeproj + Info.plist
#   5. xcodebuild                        -> Codescribe.app
#   6. embed runtime artifacts so the bundle is self-contained
#   7. sign with a stable identifier so macOS TCC grants survive rebuilds
#
# Usage:
#   scripts/build-app.sh [debug|local-release|release]
#   scripts/build-app.sh --stage-agent-bridge <destination> [bundle-version]
#
# Env toggles:
#   SKIP_XCODEBUILD=1   stop after xcodegen (verifies stages 1-4 without Xcode)
#   CODE_SIGNING_ALLOWED=YES|NO   passed through to xcodebuild (default NO)
#   CODESCRIBE_BUNDLE_EMBEDDER=1                     bundle MiniLM as an app resource (off by default)
#   CODESCRIBE_EMBEDDER_BUNDLE_SOURCE=/path/to/model  explicit MiniLM resource source
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

stage_agent_bridge() {
  local destination="$1"
  local bundle_version="$2"
  local publisher="${3:-}"
  python3 - "$REPO_ROOT" "$destination" "$bundle_version" "$publisher" <<'PY'
import hashlib
import json
import os
import shutil
import stat
import subprocess
import re
import sys
from pathlib import Path, PurePosixPath

repo = Path(sys.argv[1]).resolve()
destination = Path(sys.argv[2]).resolve()
bundle_version = sys.argv[3]
publisher = Path(sys.argv[4]) if sys.argv[4] else None
if publisher is not None and (
    not publisher.is_absolute() or not publisher.is_file()
    or publisher.is_symlink() or not os.access(publisher, os.X_OK)
):
    raise SystemExit("canonical publisher must be an ordinary executable artifact")
skill_source = repo / "skills" / "codescribe"
helper_source = repo / "scripts" / "bus-demux.py"
entrypoints = [helper_source, repo / "scripts" / "cs-bus", repo / "scripts" / "cs-say"]
if not (skill_source / "SKILL.md").is_file() or not all(path.is_file() for path in entrypoints):
    raise SystemExit("agent bridge source is incomplete")
if (
    destination == destination.parent
    or destination == Path.home()
    or destination == repo
    or repo.is_relative_to(destination)
):
    raise SystemExit(f"refusing unsafe agent bridge destination: {destination}")
source_paths = [skill_source, *entrypoints, *skill_source.rglob("*")]
source_symlinks = [path for path in source_paths if path.is_symlink()]
if source_symlinks:
    raise SystemExit(
        "agent bridge source may not contain symlinks: "
        + ", ".join(str(path) for path in source_symlinks)
    )

stage = destination.parent / f".{destination.name}.stage-{os.getpid()}"
backup = destination.parent / f".{destination.name}.backup-{os.getpid()}"
for scratch in (stage, backup):
    if scratch.exists():
        shutil.rmtree(scratch)
stage.mkdir(parents=True, mode=0o755)
# Finder droppings are not payload. Unfiltered, `.DS_Store` lands in the
# signed bundle WITH a sha256 in the manifest, so it reads as shipped
# content. Observed at bundle 0.14.1 and reproduced at 0.15.0.
shutil.copytree(
    skill_source,
    stage / "skills" / "codescribe",
    symlinks=True,
    ignore=shutil.ignore_patterns(".DS_Store"),
)
(stage / "bin").mkdir(mode=0o755)
for source in entrypoints:
    target = stage / "bin" / source.name
    shutil.copy2(source, target)
    target.chmod(0o755)
if publisher is not None:
    shutil.copy2(publisher, stage / "bin" / "codescribe")
    (stage / "bin" / "codescribe").chmod(0o755)

files = []
for path in sorted(candidate for candidate in stage.rglob("*") if candidate.is_file()):
    if path.is_symlink():
        raise SystemExit(f"agent bridge payload may not contain symlinks: {path}")
    relative = PurePosixPath(path.relative_to(stage).as_posix())
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    mode = stat.S_IMODE(path.stat().st_mode)
    files.append({
        "path": str(relative),
        "sha256": digest,
        "bytes": path.stat().st_size,
        "mode": f"{mode:04o}",
    })

manifest = {
    "schema": "codescribe.agent-bridge.bundle.v1",
    "bundle_version": bundle_version,
    "helper": "bin/bus-demux.py",
    "skill": "skills/codescribe",
    "files": files,
}
skill_header = (skill_source / "SKILL.md").read_text(encoding="utf-8")
helper_version = re.search(r'^\s+version:\s*"([^"\n]+)"', skill_header, re.MULTILINE)
manifest["helper_version"] = helper_version.group(1) if helper_version else bundle_version
try:
    manifest["source_commit"] = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], stderr=subprocess.DEVNULL, text=True
    ).strip()
    manifest["source_dirty"] = bool(subprocess.check_output(
        ["git", "-C", str(repo), "status", "--porcelain"], stderr=subprocess.DEVNULL, text=True
    ).strip())
except (OSError, subprocess.CalledProcessError):
    manifest["source_commit"] = None
    manifest["source_dirty"] = None
(stage / "manifest.json").write_text(
    json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
(stage / "manifest.json").chmod(0o644)

try:
    if destination.exists():
        os.replace(destination, backup)
    os.replace(stage, destination)
    if backup.exists():
        shutil.rmtree(backup)
except BaseException:
    if destination.exists() and backup.exists():
        shutil.rmtree(destination)
    if backup.exists():
        os.replace(backup, destination)
    raise
finally:
    if stage.exists():
        shutil.rmtree(stage)
PY
}

if [[ "${1:-}" == "--stage-agent-bridge" ]]; then
  if [[ -z "${2:-}" ]]; then
    echo "usage: $0 --stage-agent-bridge <destination> [bundle-version] [publisher-artifact]" >&2
    exit 2
  fi
  BRIDGE_STAGE_VERSION="${3:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)}"
  stage_agent_bridge "$2" "$BRIDGE_STAGE_VERSION" "${4:-}"
  echo "==> Agent bridge staged: $2 (v$BRIDGE_STAGE_VERSION)"
  exit 0
fi

PROFILE="${1:-debug}"
case "$PROFILE" in
  debug)
    CONFIG="Debug"
    CARGO_PROFILE_ARGS=()
    ;;
  local-release)
    CONFIG="Release"
    CARGO_PROFILE_ARGS=(--profile local-release)
    ;;
  release)
    CONFIG="Release"
    CARGO_PROFILE_ARGS=(--release)
    ;;
  *) echo "usage: $0 [debug|local-release|release]" >&2; exit 2 ;;
esac

# Resolve before Cargo as well as Xcode: both halves use the same build bit.
CS_DEVELOPER_SURFACE=0
if [[ "$PROFILE" != "release" && "${CODESCRIBE_DEVELOPER_SURFACE:-0}" == "1" ]]; then
  CS_DEVELOPER_SURFACE="$(./scripts/developer-surface-gate.sh)"
  if [[ "$CS_DEVELOPER_SURFACE" != "1" ]]; then
    echo "Developer build requires verified private Voice Lab access and its settings pack." >&2
    exit 1
  fi
fi
export CODESCRIBE_DEVELOPER_SURFACE="$CS_DEVELOPER_SURFACE"

# ── Preflight: a clean checkout on a fresh Mac otherwise dies deep in the
# pipeline with a cryptic "command not found". Fail early, actionably.
require() {
  command -v "$1" >/dev/null 2>&1 || { echo "error: '$1' not found — $2" >&2; exit 1; }
}
require python3 "install Python 3 for Cargo metadata and binding normalization"
require cargo    "install the Rust toolchain: https://rustup.rs"
require xcodegen "the app's .xcodeproj is generated, not committed: brew install xcodegen"
if [ "${SKIP_XCODEBUILD:-0}" != "1" ]; then
  require xcodebuild "install Xcode (App Store), then: sudo xcodebuild -runFirstLaunch"
  require swiftc "install Xcode command line tools: xcode-select --install"
fi

# Ask Cargo itself: environment, ancestor/repo config and Cargo home all share
# the same authority as the builds below. Never fall back to a local target.
if ! TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
value = json.load(sys.stdin)["target_directory"]
if not isinstance(value, str) or not value.startswith("/") or any(c in value for c in "\\\"\n\r"):
    raise SystemExit("invalid Cargo target_directory (expected an absolute path)")
print(value)
')"; then
  echo "error: cannot resolve Cargo artifact root via cargo metadata" >&2
  exit 1
fi
TARGET_DIR="$TARGET_ROOT/$PROFILE"

resolve_embedder_source() {
  local explicit="${CODESCRIBE_EMBEDDER_BUNDLE_SOURCE:-${CODESCRIBE_EMBEDDER_PATH:-}}"
  if [[ -n "$explicit" && -f "$explicit/config.json" && -f "$explicit/tokenizer.json" && -f "$explicit/model.safetensors" ]]; then
    printf '%s\n' "$explicit"
    return 0
  fi

  local repo="${CODESCRIBE_EMBEDDER_REPO:-sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2}"
  local repo_dir="models--${repo//\//--}"
  local cache_roots=()
  [[ -n "${CODESCRIBE_HF_CACHE:-}" ]] && cache_roots+=("$CODESCRIBE_HF_CACHE")
  [[ -n "${HUGGINGFACE_HUB_CACHE:-}" ]] && cache_roots+=("$HUGGINGFACE_HUB_CACHE")
  [[ -n "${HF_HUB_CACHE:-}" ]] && cache_roots+=("$HF_HUB_CACHE")
  [[ -n "${HF_HOME:-}" ]] && cache_roots+=("$HF_HOME/hub")
  cache_roots+=("$HOME/.cache/huggingface/hub")
  cache_roots+=("$HOME/.codescribe/embeddings" "$HOME/.codescribe/embeddings/hub")

  local cache snapshot
  for cache in "${cache_roots[@]}"; do
    [[ -d "$cache/$repo_dir/snapshots" ]] || continue
    for snapshot in "$cache/$repo_dir/snapshots"/*; do
      if [[ -f "$snapshot/config.json" && -f "$snapshot/tokenizer.json" && -f "$snapshot/model.safetensors" ]]; then
        printf '%s\n' "$snapshot"
        return 0
      fi
    done
  done
  return 1
}

# MiniLM ships only when a build explicitly asks for it. No runtime path in the
# app, the FFI or the sidecars calls `embedder::*` — the module's only callers
# are tests, `examples/roundtrip_live.rs` and `core/examples/lexicon_gate_calibration.rs`,
# and `core/quality/supervisor.rs` merely names `semantic_cosine` in a finding
# spec that nothing computes. A 471 MB weight file that nothing loads is not a
# payload, so the public bundle stops carrying it. Ask for it with
# CODESCRIBE_BUNDLE_EMBEDDER=1 (build-dmg.sh: --bundle-embedder) when a semantic
# gate actually lands, and resolve it from the HF cache for CLI/test lanes.
EMBEDDER_RUNTIME_SOURCE=""
if [[ "${SKIP_XCODEBUILD:-0}" != "1" \
   && "${CODESCRIBE_EMBED_EMBEDDER:-0}" != "1" \
   && "${CODESCRIBE_BUNDLE_EMBEDDER:-0}" == "1" ]]; then
  if ! EMBEDDER_RUNTIME_SOURCE="$(resolve_embedder_source)"; then
    echo "error: CODESCRIBE_BUNDLE_EMBEDDER=1 but no MiniLM found; run 'make download-embedder' or set CODESCRIBE_EMBEDDER_BUNDLE_SOURCE" >&2
    exit 1
  fi
fi

SCHEME="Codescribe"
BRIDGE_DIR="macos/Codescribe/Bridge"
DYLIB="$TARGET_DIR/libcodescribe_ffi.dylib"
BINDGEN="$TARGET_DIR/uniffi-bindgen"
STT_BRIDGE_SRC="core/stt/apple_stt/codescribe-stt-bridge.swift"
STT_BRIDGE_BIN="$TARGET_DIR/codescribe-stt-bridge"
STT_SIDECAR_BIN="$TARGET_DIR/codescribe-stt-sidecar"

# ── Build provenance (Pensieve-style) ───────────────────────────────────────
# Stamp MUST be computed BEFORE cargo/uniffi/xcodegen. Those steps can replace
# generated Bridge Swift and leave local noise (DMG .sha256, scratch files).
# About panel must show the commit that was checked out — not "-dirty" because a
# later stage regenerated UniFFI or an untracked artifact sat in the tree.
#
# Dirty rule (honest product truth):
#   - only TRACKED files (ignore untracked operator junk)
#   - exclude UniFFI-generated Bridge (normalized, installed only on byte changes)
#   - if remaining porcelain is non-empty → append -dirty
STAMP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -1)"
if git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  STAMP_COMMIT="$(git -C "$REPO_ROOT" rev-parse --short=9 HEAD)"
  # Tracked-only; drop Bridge paths (UniFFI generation mid-build). Untracked junk
  # (local *.dmg.sha256, scratch files) is ignored on purpose.
  if git -C "$REPO_ROOT" status --porcelain --untracked-files=no --ignore-submodules=none \
    | grep -Ev 'macos/Codescribe/Bridge/|Codescribe/Bridge/' \
    | grep -q .
  then
    STAMP_COMMIT="${STAMP_COMMIT}-dirty"
  fi
  STAMP_BUILD_NUM="$(git -C "$REPO_ROOT" rev-list --count HEAD)"
else
  STAMP_COMMIT="nogit"
  STAMP_BUILD_NUM="0"
fi
STAMP_BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "==> stamp (pre-build): v${STAMP_VERSION} build ${STAMP_BUILD_NUM} commit ${STAMP_COMMIT} built ${STAMP_BUILT_AT}"

echo "==> [1/7] Building codescribe-ffi ($PROFILE)"
# Verify emitted artifact receipts before any consumer can touch stale files.
# A configured build.target puts artifacts in a triple subdirectory. This app
# pipeline is host-only: reject that layout, including an explicit host triple,
# rather than executing a cross-built bindgen or linking an unrelated host dylib.
build_cargo_artifacts() {
  local package="$1" receipts
  shift
  if [ "$PROFILE" = "local-release" ]; then
    receipts="$(CODESCRIBE_LOCAL_INSTALL=1 cargo build -p "$package" "$@" ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} --message-format=json-render-diagnostics)" || return $?
  else
    receipts="$(env -u CODESCRIBE_LOCAL_INSTALL cargo build -p "$package" "$@" ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} --message-format=json-render-diagnostics)" || return $?
  fi
  printf '%s\n' "$receipts" | python3 -c '
import json, sys
from pathlib import Path
root, package = Path(sys.argv[1]), sys.argv[2]
expected = ({"codescribe_ffi": "libcodescribe_ffi.dylib", "uniffi-bindgen": "uniffi-bindgen"}
            if package == "codescribe-ffi" else {"codescribe": "codescribe"}
            if package == "codescribe" else {"codescribe-stt-sidecar": "codescribe-stt-sidecar"})
seen = set()
finished = False
for line in sys.stdin:
    if not line.startswith("{"):
        continue
    message = json.loads(line)
    if message.get("reason") == "build-finished":
        finished = message.get("success") is True
    if message.get("reason") != "compiler-artifact":
        continue
    name = message.get("target", {}).get("name")
    if name not in expected:
        continue
    if name == "codescribe" and "bin" not in message.get("target", {}).get("kind", []):
        continue
    path = root / expected[name]
    emitted = message.get("filenames", []) if name == "codescribe_ffi" else [message.get("executable")]
    if str(path) not in emitted or not path.is_file():
        raise SystemExit(f"error: Cargo artifact for {name} is not {path}; explicit build.target/cross-compilation is unsupported by build-app; remove that configuration for a host build")
    seen.add(name)
if not finished or seen != set(expected):
    raise SystemExit(f"error: incomplete Cargo artifact receipts for {package}; refusing stale local artifacts")
' "$TARGET_DIR" "$package"
}
build_cargo_artifacts codescribe-ffi
build_cargo_artifacts codescribe-core --bin codescribe-stt-sidecar
if [[ "${SKIP_XCODEBUILD:-0}" != "1" ]]; then
  build_cargo_artifacts codescribe --bin codescribe
fi

echo "==> [2/7] Rewriting dylib install_name to @rpath (relocatable bundle)"
install_name_tool -id @rpath/libcodescribe_ffi.dylib "$DYLIB"

echo "==> [3/7] Generating Swift bindings via uniffi-bindgen"
# Keep source timestamps tied to byte changes so Xcode can reuse unchanged
# objects/extraction data without l10n-sync mistaking regeneration for an edit.
bash "$REPO_ROOT/scripts/lib/generate-swift-bindings.sh" "$BINDGEN" "$DYLIB" "$BRIDGE_DIR"

echo "==> [4/7] Generating Xcode project (xcodegen)"
( cd macos && xcodegen generate )

if [ "${SKIP_XCODEBUILD:-0}" = "1" ]; then
  echo "==> SKIP_XCODEBUILD=1 — stopping after xcodegen (stages 1-4 verified)."
  exit 0
fi

echo "==> [5/7] Building app (xcodebuild, $CONFIG)"
echo "    stamp: v${STAMP_VERSION} build ${STAMP_BUILD_NUM} commit ${STAMP_COMMIT} built ${STAMP_BUILT_AT}"
echo "    developer surface: ${CS_DEVELOPER_SURFACE}"
DERIVED="$REPO_ROOT/macos/build"
# ONLY_ACTIVE_ARCH: cargo emits a single-arch libcodescribe_ffi.dylib, so a
# universal (x86_64+arm64) Release link dies on missing Rust symbols.
# LIBRARY_SEARCH_PATHS must follow the selected Cargo profile. Xcode's Debug /
# Release configs cannot distinguish distribution `release` from the optimized
# `local-release` Cargo profile because both intentionally use Release Swift.
# Xcode 27 ld rejects rustc-stripped dylibs (LINKEDIT string pool). Daily
# local-release now ships unstripped. If the selected toolchain still
# refuses the dylib and a stable Xcode.app exists, retry that linker.
run_app_xcodebuild() {
  xcodebuild -project macos/Codescribe.xcodeproj \
    -scheme "$SCHEME" -configuration "$CONFIG" \
    -derivedDataPath "$DERIVED" \
    ONLY_ACTIVE_ARCH=YES \
    LIBRARY_SEARCH_PATHS="\"$TARGET_DIR\"" \
    CODE_SIGNING_ALLOWED="${CODE_SIGNING_ALLOWED:-NO}" \
    MARKETING_VERSION="$STAMP_VERSION" \
    CURRENT_PROJECT_VERSION="$STAMP_BUILD_NUM" \
    CS_BUILD_COMMIT="$STAMP_COMMIT" \
    CS_BUILT_AT="$STAMP_BUILT_AT" \
    SPARKLE_ED_PUBLIC_KEY="${SPARKLE_ED_PUBLIC_KEY:-}" \
    CS_DEVELOPER_SURFACE="${CS_DEVELOPER_SURFACE:-0}" \
    build
}

XCODEBUILD_LOG="$(mktemp)"
if ! run_app_xcodebuild > >(tee "$XCODEBUILD_LOG") 2>&1; then
  if grep -q "mis-aligned LINKEDIT" "$XCODEBUILD_LOG" \
    && [[ -d /Applications/Xcode.app ]] \
    && [[ "${DEVELOPER_DIR:-}" != /Applications/Xcode.app* ]]; then
    echo "==> beta ld rejected the Rust dylib; retrying with /Applications/Xcode.app"
    DEVELOPER_DIR=/Applications/Xcode.app run_app_xcodebuild
  else
    rm -f "$XCODEBUILD_LOG"
    exit 65
  fi
fi
rm -f "$XCODEBUILD_LOG"

APP="$DERIVED/Build/Products/$CONFIG/$SCHEME.app"
echo "==> [6/7] Embedding runtime artifacts into $SCHEME.app"
FRAMEWORKS="$APP/Contents/Frameworks"
MACOS_DIR="$APP/Contents/MacOS"
mkdir -p "$FRAMEWORKS" "$MACOS_DIR"
cp "$DYLIB" "$FRAMEWORKS/"
cp "$STT_SIDECAR_BIN" "$MACOS_DIR/codescribe-stt-sidecar"
chmod 755 "$MACOS_DIR/codescribe-stt-sidecar"
if [[ -n "$EMBEDDER_RUNTIME_SOURCE" ]]; then
  EMBEDDER_BUNDLE_DIR="$APP/Contents/Resources/models/embedder"
  mkdir -p "$EMBEDDER_BUNDLE_DIR"
  cp -L "$EMBEDDER_RUNTIME_SOURCE/config.json" "$EMBEDDER_BUNDLE_DIR/config.json"
  cp -L "$EMBEDDER_RUNTIME_SOURCE/tokenizer.json" "$EMBEDDER_BUNDLE_DIR/tokenizer.json"
  cp -L "$EMBEDDER_RUNTIME_SOURCE/model.safetensors" "$EMBEDDER_BUNDLE_DIR/model.safetensors"
  chmod 644 "$EMBEDDER_BUNDLE_DIR/config.json" "$EMBEDDER_BUNDLE_DIR/tokenizer.json" "$EMBEDDER_BUNDLE_DIR/model.safetensors"
  echo "    MiniLM runtime resource bundled (CODESCRIBE_BUNDLE_EMBEDDER=1)."
else
  # Xcode reuses its product directory, so a bundle built earlier WITH the
  # weights keeps them until something deletes them. Copying is not the only
  # half of the contract — a build that does not ask for MiniLM must strip a
  # stale copy, or the flag silently ships 471 MB anyway (caught by
  # verify-dmg-payload.sh on 2026-09-18, before this line existed).
  rm -rf "$APP/Contents/Resources/models/embedder"
  rmdir "$APP/Contents/Resources/models" 2>/dev/null || true
  if [[ "${CODESCRIBE_EMBED_EMBEDDER:-0}" == "1" ]]; then
    echo "    MiniLM is compiled into the binary by explicit CODESCRIBE_EMBED_EMBEDDER=1."
  else
    echo "    MiniLM not bundled (no runtime consumer); stale copies stripped; CLI/tests use the HF cache."
  fi
fi
# Only the two non-secret engine defaults belong in the signed app bundle.
# The app owns merging them into settings.json at launch, with a backup.
python3 - "$APP/Contents/Resources/operator-pack/settings.json" "${CODESCRIBE_VOICE_LAB_SRC:-$REPO_ROOT/../voice-lab}" "$HOME/.codescribe/src/voice-lab" <<'PY_PACK'
import json
import sys
from pathlib import Path

dest, *roots = map(Path, sys.argv[1:])
if dest.exists():
    dest.unlink()
for pack in (pack for root in roots for pack in sorted((root / "examples").glob("*/settings.json"))):
    if not (pack.parent / "keys").is_dir():
        continue
    source = json.loads(pack.read_text()).get("speech", {}).get("engine", {})
    engine = {k: source[k] for k in ("cloud_transcription_endpoint", "asr_mode")
              if isinstance(source.get(k), str) and source[k].strip()}
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps({"speech": {"engine": engine}}, indent=2) + "\n")
    break
PY_PACK

AGENT_BRIDGE_BUNDLE_DIR="$APP/Contents/Resources/agent-bridge"
stage_agent_bridge "$AGENT_BRIDGE_BUNDLE_DIR" "$STAMP_VERSION" "$TARGET_DIR/codescribe"
echo "    Agent bridge skill tree + session helper bundled at Contents/Resources/agent-bridge."
STT_BRIDGE_BUNDLED=0
# Same host-triple pin as Makefile ENGINE_BRIDGE_TARGET (W0-B / S-1): avoid
# inheriting the builder's macosxN.0 so bundled bridges match CI/dev hosts.
STT_BRIDGE_TARGET="${CODESCRIBE_STT_BRIDGE_TARGET:-arm64-apple-macos26.0}"
rm -f "$STT_BRIDGE_BIN" "$MACOS_DIR/codescribe-stt-bridge"
if swiftc -O -target "$STT_BRIDGE_TARGET" -o "$STT_BRIDGE_BIN" "$STT_BRIDGE_SRC"; then
  cp "$STT_BRIDGE_BIN" "$MACOS_DIR/"
  chmod 755 "$MACOS_DIR/codescribe-stt-bridge"
  STT_BRIDGE_BUNDLED=1
else
  echo "warning: Apple STT bridge helper skipped; this SDK may not include SpeechAnalyzer/SpeechTranscriber." >&2
  echo "warning: Codescribe.app will build without the bundled helper and use runtime STT fallback resolution." >&2
fi

# Install/release lanes only (`make install-app`, scripts/build-dmg.sh set
# CODESCRIBE_INSTALL_LANE=1): the installed app refuses a second generation.
# Dev builds and the XCTest host (same target, distinct Debug bundle id) stay
# launchable while an installed build runs. Stamped here, before stage 7,
# because an Info.plist edit after codesign breaks the seal.
INSTALL_LANE="${CODESCRIBE_INSTALL_LANE:-0}"
if [ "$INSTALL_LANE" = "1" ]; then
  "$REPO_ROOT/scripts/lib/stamp-single-instance.sh" "$APP"
fi

# Ad-hoc sign the finished bundle with a STABLE identifier so macOS TCC
# (Accessibility / Input Monitoring) keeps its grant across rebuilds instead of
# re-prompting every time an unsigned binary's cdhash changes. Use the built
# plist identity for this configuration; `--deep` covers the embedded dylib.
BUNDLE_ID="${CODESCRIBE_BUNDLE_ID:-$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")}"
# Prefer a REAL signing identity (Developer ID / Apple Development). Its designated
# requirement is certificate-based, so a TCC grant (Accessibility / Input
# Monitoring) survives rebuilds. Ad-hoc (`--sign -`) is cdhash-based, so the grant
# dies on every rebuild — fall back to it only when no real identity exists.
SIGN_ID="${CODESCRIBE_CODESIGN_IDENTITY:-}"
if [ -z "$SIGN_ID" ] || [ "$SIGN_ID" = "-" ]; then
  SIGN_ID="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
  [ -z "$SIGN_ID" ] && SIGN_ID="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
fi
# Resource executables are outside codesign's automatic nested-code paths.
# Sign the canonical publisher explicitly before the outer resource seal.
codesign --force --sign "${SIGN_ID:--}" --identifier "$BUNDLE_ID.publisher" "$AGENT_BRIDGE_BUNDLE_DIR/bin/codescribe"
if [ -n "$SIGN_ID" ]; then
  echo "==> [7/7] Signing $SCHEME.app with stable identity: $SIGN_ID"
  codesign --force --deep --sign "$SIGN_ID" --identifier "$BUNDLE_ID" "$APP"
else
  echo "==> [7/7] Ad-hoc signing $SCHEME.app (no stable identity — TCC re-grants per build)"
  codesign --force --deep --sign - --identifier "$BUNDLE_ID" "$APP"
fi
python3 "$REPO_ROOT/scripts/lib/refresh-agent-publisher-manifest.py" "$AGENT_BRIDGE_BUNDLE_DIR"
# Publisher signing changes its Mach-O bytes. Seal its refreshed digest
# with the outer signature without signing that child a second time.
codesign --force --sign "${SIGN_ID:--}" --identifier "$BUNDLE_ID" "$APP"
if [ "$INSTALL_LANE" = "1" ]; then
  # The stamped plist must sit inside the seal, not beside it.
  codesign --verify --deep --strict "$APP"
  echo "    codesign --verify --deep --strict: ok (single-instance stamp is sealed)"
fi

echo "==> App built: $APP"
echo "    (portability: dylib is @rpath-relative and embedded; project.yml adds"
echo "     @executable_path/../Frameworks to the app runpath.)"
if [ "$STT_BRIDGE_BUNDLED" = "1" ]; then
  echo "    Apple STT bridge is bundled beside the app executable in Contents/MacOS."
else
  echo "    Apple STT bridge is not bundled; runtime resolution will use env/PATH/fallback."
fi
echo "    Whisper tail-patch sidecar is bundled beside the app executable in Contents/MacOS."
