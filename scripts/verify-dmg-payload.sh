#!/bin/bash
# verify-dmg-payload.sh — fail-closed DMG payload gate
#
# Signed ≠ complete. Regression 0.13.2 (≈89 MB DMG, no MiniLM)
# passed codesign + notarize + staple. This gate is the defense class that
# refuses an incomplete public artifact.
#
# Payload contract (core/build.rs + Makefile release-codescribe):
#   slim: Silero VAD embedded + MiniLM signed runtime resource; Whisper runtime
#   full: Silero + Whisper embedded + MiniLM signed runtime resource
# MiniLM is checked directly under Contents/Resources/models/embedder; Cargo
# target size is no longer used as a proxy for product completeness.
#
# Probe mechanism (calibrated 2026-08-04 on fixtures):
#   Silero/optional Whisper land in libcodescribe_ffi.dylib. MiniLM is a normal
#   signed app resource loaded at runtime. Hard file-presence/size checks plus
#   DMG/dylib floors are the fail-closed signal:
#     BAD 0.13.2:      dmg≈85 MB,  dylib≈29 MB
#     expected slim:    dmg≈501 MB, dylib≈30 MB, MiniLM≈471 MB resource
#     expected full:    dmg≈1.3 GB, dylib≈800+ MB, MiniLM≈471 MB resource
#
# Usage:
#   ./scripts/verify-dmg-payload.sh <dmg> --variant slim|full --version X.Y.Z
#   ./scripts/verify-dmg-payload.sh <dmg> --variant slim --version X.Y.Z --skip-notary
#   SPARKLE_ED_PUBLIC_KEY=<expected> ./scripts/verify-dmg-payload.sh ...
#
# Exit: 0 = accepted; non-zero = refuse (fail-closed).
#
# 𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

set -euo pipefail

# ── Calibrated thresholds (bytes) ────────────────────────────────────────────
# The slim dylib contains code + Silero only; MiniLM has its own direct resource
# gate below. Keep a low engine-presence floor instead of forcing model bytes
# through Cargo artifacts.
readonly SLIM_DMG_MIN=$((400 * 1024 * 1024))
readonly SLIM_DYLIB_MIN=$((20 * 1024 * 1024))
# Whisper large-v3-turbo + code/Silero remains in the full dylib.
readonly FULL_DMG_MIN=$((1000 * 1024 * 1024))
readonly FULL_DYLIB_MIN=$((700 * 1024 * 1024))
readonly EMBEDDER_WEIGHTS_MIN=$((400 * 1024 * 1024))

# Soft markers (advisory + secondary). Size is primary.
readonly MARKER_MINILM='paraphrase-multilingual-MiniLM'
readonly MARKER_SILERO='silero'

DMG=""
VARIANT=""
VERSION=""
SKIP_NOTARY=0
MOUNT_POINT=""
FAILED=0
FAILURES=()

usage() {
  cat <<EOF
Usage: $0 <dmg> --variant slim|full --version X.Y.Z [--skip-notary]

Fail-closed payload gate for Codescribe release DMGs.

  <dmg>               Path to Codescribe_*.dmg
  --variant slim|full slim = Silero + MiniLM resource; full = +Whisper embedded
  --version X.Y.Z     Expected CFBundleShortVersionString
  --skip-notary       Skip stapler + spctl (pre-notarization smoke)

SPARKLE_ED_PUBLIC_KEY, when set, must match the non-empty SUPublicEDKey in the
mounted app. An empty bundle key is always refused.

Exit 0 only when every assert passes. Detach is always attempted (trap).
EOF
}

fail() {
  local msg="$1"
  FAILED=1
  FAILURES+=("$msg")
  echo "✗ $msg" >&2
}

ok() {
  echo "  ✓ $1"
}

human_mb() {
  # Prefer integer MB for thresholds; accept both bash arithmetic and awk.
  awk -v b="$1" 'BEGIN { printf "%.1f" , b / (1024 * 1024) }'
}

# Detach always — trap body inlined so shellcheck sees the cleanup path.
trap '
  if [[ -n "${MOUNT_POINT:-}" && -d "${MOUNT_POINT}" ]]; then
    hdiutil detach "$MOUNT_POINT" -quiet 2>/dev/null \
      || hdiutil detach "$MOUNT_POINT" -force -quiet 2>/dev/null \
      || true
    MOUNT_POINT=""
  fi
' EXIT INT TERM

# ── Args ─────────────────────────────────────────────────────────────────────
if [[ $# -lt 1 ]]; then
  usage
  exit 2
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --variant)
      VARIANT="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --skip-notary)
      SKIP_NOTARY=1
      shift
      ;;
    --*)
      echo "Unknown option: $1" >&2
      usage
      exit 2
      ;;
    *)
      if [[ -z "$DMG" ]]; then
        DMG="$1"
        shift
      else
        echo "Unexpected argument: $1" >&2
        usage
        exit 2
      fi
      ;;
  esac
done

if [[ -z "$DMG" || -z "$VARIANT" || -z "$VERSION" ]]; then
  echo "ERROR: <dmg>, --variant, and --version are required" >&2
  usage
  exit 2
fi

case "$VARIANT" in
  slim|full) ;;
  *)
    echo "ERROR: --variant must be slim or full (got: $VARIANT)" >&2
    exit 2
    ;;
esac

if [[ ! -f "$DMG" ]]; then
  echo "ERROR: DMG not found: $DMG" >&2
  exit 2
fi

# Resolve to absolute for codesign/spctl/shasum friendliness.
if [[ "$DMG" != /* ]]; then
  DMG="$(cd "$(dirname "$DMG")" && pwd)/$(basename "$DMG")"
fi

echo "═══════════════════════════════════════════════════════════"
echo "  Codescribe DMG payload gate (fail-closed)"
echo "═══════════════════════════════════════════════════════════"
echo "  DMG:     $DMG"
echo "  Variant: $VARIANT"
echo "  Version: $VERSION"
echo "  Notary:  $([[ $SKIP_NOTARY -eq 1 ]] && echo skip || echo required)"
echo "───────────────────────────────────────────────────────────"

# ── 1. DMG size floor (fast pre-check) ───────────────────────────────────────
echo ""
echo "▶ DMG size floor ($VARIANT)"
DMG_BYTES=$(stat -f%z "$DMG")
case "$VARIANT" in
  slim)
    MIN_DMG=$SLIM_DMG_MIN
    ;;
  full)
    MIN_DMG=$FULL_DMG_MIN
    ;;
esac
if (( DMG_BYTES < MIN_DMG )); then
  fail "DMG too small for $VARIANT: $(human_mb "$DMG_BYTES") MB < $(human_mb "$MIN_DMG") MB floor — missing MiniLM resource and/or Whisper payload"
else
  ok "DMG size $(human_mb "$DMG_BYTES") MB ≥ $(human_mb "$MIN_DMG") MB"
fi

# ── 2. SHA-256 sidecar ───────────────────────────────────────────────────────
echo ""
echo "▶ SHA-256 sidecar"
SHA_SIDE="${DMG}.sha256"
if [[ ! -f "$SHA_SIDE" ]]; then
  fail "Missing SHA-256 sidecar: $SHA_SIDE (build-dmg.sh always writes one)"
else
  # shasum -c expects the file listed inside the sidecar to be relative to cwd
  # of the check, or an absolute path. Sidecars use bare basename.
  if (
    cd "$(dirname "$DMG")"
    shasum -a 256 -c "$(basename "$SHA_SIDE")" >/dev/null 2>&1
  ); then
    ok "sha256 matches $(basename "$SHA_SIDE")"
  else
    fail "SHA-256 mismatch against $SHA_SIDE"
  fi
fi

# ── 3. codesign (DMG) ────────────────────────────────────────────────────────
echo ""
echo "▶ codesign --verify (DMG)"
if codesign --verify --verbose=2 "$DMG" >/dev/null 2>&1; then
  ok "DMG signature present"
else
  # Ad-hoc unsigned dev DMGs fail here — correct fail-closed for release gate.
  fail "codesign --verify failed on DMG (unsigned or invalid signature)"
fi

# ── 4. Notarization / Gatekeeper (optional skip) ─────────────────────────────
if [[ $SKIP_NOTARY -eq 0 ]]; then
  echo ""
  echo "▶ stapler validate"
  if xcrun stapler validate "$DMG" >/dev/null 2>&1; then
    ok "stapler ticket present"
  else
    fail "xcrun stapler validate failed (not stapled / not notarized)"
  fi

  echo ""
  echo "▶ spctl Gatekeeper"
  SPCTL_OUT=""
  if SPCTL_OUT=$(spctl -a -t open --context context:primary-signature -v "$DMG" 2>&1); then
    if echo "$SPCTL_OUT" | grep -qi 'accepted'; then
      ok "spctl accepted (primary-signature)"
    else
      fail "spctl did not report accepted: $SPCTL_OUT"
    fi
  else
    fail "spctl rejected DMG: $SPCTL_OUT"
  fi
else
  echo ""
  echo "▶ notarization checks skipped (--skip-notary)"
fi

# ── 5. Mount + inspect bundle ────────────────────────────────────────────────
echo ""
echo "▶ mount + inspect payload"
ATTACH_OUT=""
ATTACH_OUT=$(hdiutil attach -readonly -nobrowse -mountrandom /tmp "$DMG" 2>&1) || {
  fail "hdiutil attach failed: $ATTACH_OUT"
  # Cannot continue payload probes without mount.
  echo ""
  echo "═══════════════════════════════════════════════════════════"
  echo "  REFUSED — ${#FAILURES[@]} failure(s)"
  for f in "${FAILURES[@]}"; do
    echo "    • $f"
  done
  echo "═══════════════════════════════════════════════════════════"
  exit 1
}
MOUNT_POINT=$(echo "$ATTACH_OUT" | awk 'END { print $NF }')
if [[ -z "$MOUNT_POINT" || ! -d "$MOUNT_POINT" ]]; then
  fail "could not resolve mount point from hdiutil output"
  echo "  attach output: $ATTACH_OUT" >&2
  exit 1
fi
ok "mounted at $MOUNT_POINT"

APP_PATH=$(find "$MOUNT_POINT" -maxdepth 2 -name '*.app' -type d 2>/dev/null | head -1 || true)
if [[ -z "$APP_PATH" || ! -d "$APP_PATH" ]]; then
  fail "no .app bundle found inside DMG"
else
  ok "app: $(basename "$APP_PATH")"
fi

# Version
if [[ -n "${APP_PATH:-}" ]]; then
  PLIST="$APP_PATH/Contents/Info.plist"
  if [[ ! -f "$PLIST" ]]; then
    fail "Info.plist missing at $PLIST"
  else
    BUNDLE_VER=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST" 2>/dev/null || true)
    if [[ -z "$BUNDLE_VER" ]]; then
      fail "CFBundleShortVersionString unreadable"
    elif [[ "$BUNDLE_VER" != "$VERSION" ]]; then
      fail "version mismatch: CFBundleShortVersionString='$BUNDLE_VER' expected='$VERSION'"
    else
      ok "CFBundleShortVersionString == $VERSION"
    fi

    SPARKLE_BUNDLE_KEY=$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$PLIST" 2>/dev/null || true)
    if [[ -z "${SPARKLE_BUNDLE_KEY//[[:space:]]/}" ]]; then
      fail "SUPublicEDKey is empty or missing — Sparkle would reject every update"
    elif [[ -n "${SPARKLE_ED_PUBLIC_KEY:-}" && "$SPARKLE_BUNDLE_KEY" != "$SPARKLE_ED_PUBLIC_KEY" ]]; then
      fail "SUPublicEDKey does not match SPARKLE_ED_PUBLIC_KEY used for this release"
    else
      ok "SUPublicEDKey is non-empty${SPARKLE_ED_PUBLIC_KEY:+ and matches release input}"
    fi

    DEV_SURFACE=$(/usr/libexec/PlistBuddy -c 'Print :CSDeveloperSurface' "$PLIST" 2>/dev/null || echo 0)
    DEV_SURFACE="${DEV_SURFACE//[[:space:]]/}"
    if [[ "$DEV_SURFACE" == "1" || "$DEV_SURFACE" == "true" || "$DEV_SURFACE" == "YES" ]]; then
      fail "CSDeveloperSurface is set — Lab must not ship in a production DMG"
    else
      ok "CSDeveloperSurface is off"
    fi
  fi

  # Deep codesign on the app bundle
  echo ""
  echo "▶ codesign --verify --deep --strict (app)"
  if codesign --verify --deep --strict "$APP_PATH" >/dev/null 2>&1; then
    ok "app signature deep/strict OK"
  else
    fail "codesign --verify --deep --strict failed on $(basename "$APP_PATH")"
  fi

  # dylib payload
  echo ""
  echo "▶ MiniLM runtime resource proof"
  EMBEDDER_DIR="$APP_PATH/Contents/Resources/models/embedder"
  EMBEDDER_WEIGHTS="$EMBEDDER_DIR/model.safetensors"
  if [[ ! -f "$EMBEDDER_DIR/config.json" || ! -f "$EMBEDDER_DIR/tokenizer.json" || ! -f "$EMBEDDER_WEIGHTS" ]]; then
    fail "MiniLM runtime resource incomplete at Contents/Resources/models/embedder"
  else
    EMBEDDER_BYTES=$(stat -f%z "$EMBEDDER_WEIGHTS")
    if (( EMBEDDER_BYTES < EMBEDDER_WEIGHTS_MIN )); then
      fail "MiniLM weights too small: $(human_mb "$EMBEDDER_BYTES") MB < $(human_mb "$EMBEDDER_WEIGHTS_MIN") MB"
    else
      ok "MiniLM runtime resource complete ($(human_mb "$EMBEDDER_BYTES") MB weights)"
    fi
  fi

  # dylib payload
  echo ""
  echo "▶ payload proof (libcodescribe_ffi.dylib, variant=$VARIANT)"
  DYLIB=$(find "$APP_PATH" -name 'libcodescribe_ffi.dylib' 2>/dev/null | head -1 || true)
  if [[ -z "$DYLIB" || ! -f "$DYLIB" ]]; then
    fail "libcodescribe_ffi.dylib missing inside app (no engine payload)"
  else
    DYLIB_BYTES=$(stat -f%z "$DYLIB")
    case "$VARIANT" in
      slim)
        MIN_DYLIB=$SLIM_DYLIB_MIN
        EXPECT_LABEL="engine code + embedded Silero (MiniLM is checked as a resource)"
        ;;
      full)
        MIN_DYLIB=$FULL_DYLIB_MIN
        EXPECT_LABEL="engine code + Silero + embedded Whisper"
        ;;
    esac

    if (( DYLIB_BYTES < MIN_DYLIB )); then
      fail "dylib=$(human_mb "$DYLIB_BYTES") MB < $(human_mb "$MIN_DYLIB") MB floor for $VARIANT — expected $EXPECT_LABEL"
    else
      ok "dylib size $(human_mb "$DYLIB_BYTES") MB ≥ $(human_mb "$MIN_DYLIB") MB ($VARIANT)"
    fi

    # Secondary string markers (do not pass alone; size already decided).
    # Use LC_ALL=C + grep -a so binary scan is cheap and shellcheck-clean.
    if grep -a -q -F "$MARKER_SILERO" "$DYLIB" 2>/dev/null \
      || grep -a -qi -F 'Silero' "$DYLIB" 2>/dev/null; then
      ok "silero marker present in dylib (secondary)"
    else
      # Silero is non-negotiable in build.rs; absence is hard fail.
      fail "silero marker absent from dylib (Silero VAD must always be embedded)"
    fi

    if grep -a -q -F "$MARKER_MINILM" "$DYLIB" 2>/dev/null; then
      ok "minilm runtime resolver marker present in dylib (secondary)"
    else
      echo "  · note: MiniLM repo marker absent from dylib — direct resource gate already applied"
    fi

    if [[ "$VARIANT" == "full" ]]; then
      # Whisper embed roughly triples dylib vs slim; floor already catches it.
      # Also refuse a "full" label on a slim-sized dylib that somehow cleared
      # a mis-set threshold.
      if (( DYLIB_BYTES < FULL_DYLIB_MIN )); then
        fail "full variant requires Whisper embed; dylib too small"
      else
        ok "full-variant Whisper size budget satisfied"
      fi
    fi
  fi
fi

# ── Verdict ──────────────────────────────────────────────────────────────────
echo ""
if [[ $FAILED -ne 0 ]]; then
  echo "═══════════════════════════════════════════════════════════"
  echo "  REFUSED — ${#FAILURES[@]} failure(s)"
  for f in "${FAILURES[@]}"; do
    echo "    • $f"
  done
  echo "═══════════════════════════════════════════════════════════"
  exit 1
fi

echo "═══════════════════════════════════════════════════════════"
echo "  ACCEPTED — $VARIANT payload complete for $VERSION"
echo "═══════════════════════════════════════════════════════════"
exit 0
