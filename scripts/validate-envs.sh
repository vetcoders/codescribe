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
ENV_EXAMPLE=".env.example"
ERRORS=0
FIX_MODE=0
CHECK_EXAMPLE=0
EMIT_ENV_PATH=""
GREP_EXCLUDES=(--exclude-dir=.git --exclude-dir=target --exclude-dir=models --exclude-dir=dist)
IGNORE_VARS="CARGO_MANIFEST_DIR OUT_DIR PROFILE"

usage() {
    cat <<EOF
Usage: $0 [--fix] [--env-example] [--env-example-path PATH] [--emit-e2e-env PATH]

  --fix               Show TOML stubs for missing variables (code scan only)
  --env-example       Validate .env.example against registry (ground truth)
  --env-example-path  Override path to env example (default: .env.example)
  --emit-e2e-env      Emit a clean env file from .env.example (used by E2E tests)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --fix)
            FIX_MODE=1
            shift
            ;;
        --env-example)
            CHECK_EXAMPLE=1
            shift
            ;;
        --env-example-path)
            ENV_EXAMPLE="$2"
            shift 2
            ;;
        --emit-e2e-env)
            EMIT_ENV_PATH="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown arg: $1"
            usage
            exit 1
            ;;
    esac
done

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

# Find all env vars used in Rust code (env::var patterns)
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
    if [[ $FIX_MODE -eq 1 ]]; then
        exit 1
    fi
else
    echo -e "${GREEN}✅ All environment variables are registered in $REGISTRY${NC}"
    echo "   Checked $(echo "$ALL_CODE_VARS" | wc -l | tr -d ' ') variables"
fi

# Validate .env.example (ground truth) if requested or when emitting E2E env
EXAMPLE_ERRORS=0
if [[ $CHECK_EXAMPLE -eq 1 || -n "$EMIT_ENV_PATH" ]]; then
    if [[ ! -f "$ENV_EXAMPLE" ]]; then
        echo -e "${RED}ERROR: env example not found: $ENV_EXAMPLE${NC}"
        exit 1
    fi

    EXAMPLE_VARS=$(grep -E '^[A-Z_][A-Z0-9_]*=' "$ENV_EXAMPLE" | sed 's/=.*//' | sort -u)
    UNREGISTERED_EXAMPLE=""
    for var in $EXAMPLE_VARS; do
        if ! echo "$REGISTERED" | grep -qx "$var"; then
            UNREGISTERED_EXAMPLE="$UNREGISTERED_EXAMPLE$var\n"
            EXAMPLE_ERRORS=$((EXAMPLE_ERRORS + 1))
        fi
    done

    if [[ $EXAMPLE_ERRORS -gt 0 ]]; then
        echo -e "${RED}❌ .env.example contains $EXAMPLE_ERRORS unknown var(s):${NC}"
        echo -e "$UNREGISTERED_EXAMPLE" | while read -r var; do
            [[ -n "$var" ]] && echo -e "   ${YELLOW}$var${NC}"
        done
    else
        echo -e "${GREEN}✅ .env.example matches registry${NC}"
    fi

    if [[ -n "$EMIT_ENV_PATH" ]]; then
        mkdir -p "$(dirname "$EMIT_ENV_PATH")" 2>/dev/null || true
        grep -E '^[A-Z_][A-Z0-9_]*=' "$ENV_EXAMPLE" > "$EMIT_ENV_PATH"
        echo "🧪 E2E env written: $EMIT_ENV_PATH"
    fi
fi

if [[ $ERRORS -gt 0 || $EXAMPLE_ERRORS -gt 0 ]]; then
    exit 1
fi
exit 0
