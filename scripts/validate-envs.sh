#!/bin/bash
# validate-envs.sh - Validate env vars against central registry
#
# Usage:
#   ./scripts/validate-envs.sh          # Check for unregistered env vars
#   ./scripts/validate-envs.sh --fix    # Show what needs to be added to registry
#
# Exit codes:
#   0 - All env vars are registered
#   1 - Found unregistered env vars (add them to docs/ENV_REGISTRY.toml)
#
# Created by M&K (c)2026 VetCoders

set -euo pipefail

REGISTRY="docs/ENV_REGISTRY.toml"
ERRORS=0
GREP_EXCLUDES=(--exclude-dir=.git --exclude-dir=target --exclude-dir=models --exclude-dir=dist)
IGNORE_VARS="CARGO_MANIFEST_DIR OUT_DIR PROFILE"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "🔍 Validating environment variables against registry..."

# Extract registered var names from TOML
if [[ ! -f "$REGISTRY" ]]; then
    echo -e "${RED}ERROR: Registry file not found: $REGISTRY${NC}"
    exit 1
fi

# Get all registered var names (including deprecated)
REGISTERED=$(grep -E '^\[vars\.' "$REGISTRY" | sed 's/\[vars\.\(.*\)\]/\1/' | sort -u)

# Find all env vars used in Rust code (std::env::var patterns)
echo "Scanning Rust code for env var usage..."
CODE_VARS=$(grep -rhoE "${GREP_EXCLUDES[@]}" 'env::var\("([A-Z_]+)"' core/ app/ bin/ 2>/dev/null | \
    sed 's/.*env::var("\([^"]*\)".*/\1/' | \
    sort -u || true)

# Find env vars in helper patterns
CODE_VARS2=$(grep -rhoE "${GREP_EXCLUDES[@]}" 'env_(bool|bool_default|u64|usize|f32|f32_clamped)\("([A-Z_]+)"' core/ app/ bin/ 2>/dev/null | \
    sed 's/.*("\([^"]*\)"/\1/' | \
    sort -u || true)

CODE_VARS3=$(grep -rhoE "${GREP_EXCLUDES[@]}" 'env_flag\("([A-Z_]+)"' core/ 2>/dev/null | \
    sed 's/env_flag("\([^"]*\)"/\1/' | \
    sort -u || true)

# Combine all found vars
ALL_CODE_VARS=$(echo -e "$CODE_VARS\n$CODE_VARS2\n$CODE_VARS3" | sort -u | grep -v '^$' || true)

# Check each code var against registry
UNREGISTERED=""
for var in $ALL_CODE_VARS; do
    if echo "$IGNORE_VARS" | grep -qw "$var"; then
        continue
    fi
    if ! echo "$REGISTERED" | grep -qx "$var"; then
        UNREGISTERED="$UNREGISTERED$var\n"
        ERRORS=$((ERRORS + 1))
    fi
done

if [[ $ERRORS -gt 0 ]]; then
    echo -e "${RED}❌ Found $ERRORS unregistered environment variable(s):${NC}"
    echo -e "$UNREGISTERED" | while read -r var; do
        [[ -n "$var" ]] && echo -e "   ${YELLOW}$var${NC}"
    done
    echo ""
    echo -e "${YELLOW}Add these to docs/ENV_REGISTRY.toml:${NC}"
    echo -e "$UNREGISTERED" | while read -r var; do
        if [[ -n "$var" ]]; then
            echo "[vars.$var]"
            echo 'default = ""'
            echo 'type = "string"'
            echo 'reload = "restart"'
            echo 'category = "unknown"'
            echo 'description = "TODO: Add description"'
            echo ""
        fi
    done
    exit 1
else
    echo -e "${GREEN}✅ All environment variables are registered in $REGISTRY${NC}"
    echo "   Checked $(echo "$ALL_CODE_VARS" | wc -l | tr -d ' ') variables"
    exit 0
fi
