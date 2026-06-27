#!/bin/bash
# CodeScribe — Mac App Store (MAS) readiness preflight
#
# READ-ONLY. Inspects the repo for state that is known-incompatible with Mac App
# Store distribution and reports each item as a blocker (P0), a warning (P1), or
# OK. It does NOT modify build behavior, entitlements, or any tracked file.
#
# The current shipping lane is Developer ID + notarization (outside the App
# Store). This script does not change that. It exists so an operator can answer
# "how far is the tree from a sandbox-clean MAS build?" without re-deriving it by
# hand. See docs/APP_STORE_READINESS.md for the full plan and citations.
#
# Usage:
#   ./scripts/appstore-preflight.sh          # human-readable report
#   ./scripts/appstore-preflight.sh && echo OK   # exit 0 only if no P0 blockers
#
# Exit code: 0 = no P0 blockers found, 1 = at least one P0 blocker.
#
# Created by M&K (c)2026 VetCoders

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
cd "$ROOT"

ENTITLEMENTS="scripts/entitlements.plist"
MAKEFILE="Makefile"
KEYCHAIN_RS="core/config/keychain.rs"
RELEASE_YML=".github/workflows/release.yml"

P0=0
P1=0

red()   { printf '\033[31m%s\033[0m\n' "$1"; }
green() { printf '\033[32m%s\033[0m\n' "$1"; }
yellow(){ printf '\033[33m%s\033[0m\n' "$1"; }

blocker() { red   "  [P0] $1"; P0=$((P0 + 1)); }
warn()    { yellow "  [P1] $1"; P1=$((P1 + 1)); }
ok()      { green "  [OK] $1"; }

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe — Mac App Store readiness preflight (read-only)"
echo "═══════════════════════════════════════════════════════════"

# 1. App Sandbox — REQUIRED for MAS, intentionally absent today.
echo ""
echo "▶ App Sandbox entitlement (com.apple.security.app-sandbox)"
if grep -q "com.apple.security.app-sandbox" "$ENTITLEMENTS" 2>/dev/null \
   && grep -A1 "com.apple.security.app-sandbox" "$ENTITLEMENTS" | grep -q "<true/>"; then
    ok "App Sandbox is enabled."
else
    blocker "App Sandbox is NOT enabled in $ENTITLEMENTS. MAS rejects builds without it."
fi

# 2. Hardened-runtime relaxations that conflict with a clean sandboxed MAS build.
echo ""
echo "▶ Sandbox-hostile hardened-runtime relaxations"
for key in \
    "com.apple.security.cs.disable-library-validation" \
    "com.apple.security.cs.allow-unsigned-executable-memory" \
    "com.apple.security.cs.allow-dyld-environment-variables"; do
    if grep -q "$key" "$ENTITLEMENTS" 2>/dev/null; then
        warn "$key present (required by embedded ML dylibs; review against MAS — see readiness doc)."
    else
        ok "$key absent."
    fi
done

# 3. Privacy manifest — required by App Store Connect since 2024-05-01.
echo ""
echo "▶ Privacy manifest (PrivacyInfo.xcprivacy)"
if find . -path ./target -prune -o -name "PrivacyInfo.xcprivacy" -print 2>/dev/null | grep -q .; then
    ok "PrivacyInfo.xcprivacy present."
else
    blocker "No PrivacyInfo.xcprivacy. Required-reason APIs (FileTimestamp via std::fs metadata) are used; MAS submission needs it."
fi

# 4. Bundle identifier consistency across the surfaces that must agree.
echo ""
echo "▶ Bundle identifier consistency"
MK_ID="$(grep -E "^CODESCRIBE_BUNDLE_ID \?=" "$MAKEFILE" 2>/dev/null | sed -E 's/.*= *//' | tr -d ' ')"
KC_ID="$(grep -oE "com\.[a-zA-Z0-9.]+codescribe[a-zA-Z0-9.]*" "$KEYCHAIN_RS" 2>/dev/null | head -1)"
YML_ID="$(grep -oE "com\.[a-zA-Z0-9.]+codescribe[a-zA-Z0-9.]*|com\.codescribe\.app" "$RELEASE_YML" 2>/dev/null | head -1)"
echo "    Makefile default : ${MK_ID:-<none>}"
echo "    keychain.rs      : ${KC_ID:-<none>}"
echo "    release.yml      : ${YML_ID:-<none>}"
if [ -n "$MK_ID" ] && [ "$MK_ID" = "$KC_ID" ] && [ "$MK_ID" = "$YML_ID" ]; then
    ok "Bundle id is consistent across Makefile, keychain, and release workflow."
else
    warn "Bundle id differs across surfaces. A MAS app record needs one canonical id (TCC + keychain identity)."
fi

# 5. Apple Events / Accessibility purpose strings — MAS review risk under sandbox.
echo ""
echo "▶ Sandbox-review-sensitive purpose strings (Makefile bundle target)"
if grep -q "NSAppleEventsUsageDescription" "$MAKEFILE" 2>/dev/null; then
    warn "NSAppleEventsUsageDescription present. Apple-events temporary exception is 'most often rejected' by App Review."
else
    ok "No NSAppleEventsUsageDescription."
fi
if grep -q "NSAccessibilityUsageDescription" "$MAKEFILE" 2>/dev/null; then
    warn "NSAccessibilityUsageDescription present. Accessibility for non-accessibility purposes risks Guideline 2.4.5 rejection."
else
    ok "No NSAccessibilityUsageDescription."
fi

# 6. App Store distribution path — none today (Developer ID only).
echo ""
echo "▶ App Store upload path"
if grep -qiE "productbuild|App Store Connect|altool|Transporter|3rd Party Mac Developer|Apple Distribution" "$MAKEFILE" "$RELEASE_YML" 2>/dev/null; then
    ok "An App Store distribution path is referenced."
else
    blocker "No App Store distribution path (productbuild/pkg, Apple Distribution cert, App Store Connect upload). Only Developer ID + notarization exists."
fi

echo ""
echo "───────────────────────────────────────────────────────────"
echo "  Summary: $P0 P0 blocker(s), $P1 P1 warning(s)"
echo "  Lane today: Developer ID + notarization (outside App Store)."
echo "  Full plan + Apple citations: docs/APP_STORE_READINESS.md"
echo "───────────────────────────────────────────────────────────"

if [ "$P0" -gt 0 ]; then
    exit 1
fi
exit 0
