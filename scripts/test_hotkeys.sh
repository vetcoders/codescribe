#!/bin/bash

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Test script for dual hotkey modes in Codescribe
# This script helps verify CTRL vs CTRL+SHIFT functionality

echo "================================================"
echo "Codescribe Dual Hotkey Mode Test"
echo "================================================"
echo ""
echo "Current configuration:"
echo "- AI_FORMATTING_ENABLED: ${AI_FORMATTING_ENABLED:-1}"
echo "- LLM_ENDPOINT: ${LLM_ENDPOINT:-http://localhost:11434/api/chat}"
echo "- LLM_FORMATTING_MODEL: ${LLM_FORMATTING_MODEL:-<unset>}"
echo "- LLM_ASSISTIVE_MODEL: ${LLM_ASSISTIVE_MODEL:-<unset>}"
echo "- USE_LOCAL_STT: ${USE_LOCAL_STT:-1}"
echo ""
echo "Testing Instructions:"
echo "================================================"
echo ""
echo "1. CTRL Mode (Formatting Only - max 512 tokens):"
echo "   - Hold CTRL and speak normally"
echo "   - Say something like: 'hmm, I wanted to write a sorting function'"
echo "   - Expected: Text cleaned up, fillers removed, punctuation fixed"
echo "   - Should NOT generate code or long responses"
echo ""
echo "2. CTRL+SHIFT Mode (Assistive AI - max 8192 tokens):"
echo "   - Hold CTRL+SHIFT and speak"
echo "   - Say: 'El Niño, write a bubble sort function in Python'"
echo "   - Expected: Full AI response with code generation"
echo "   - Can generate long, detailed responses"
echo ""
echo "3. AI_FORMATTING_ENABLED=0 (Raw only):"
echo "   - Set AI_FORMATTING_ENABLED=0 in .env"
echo "   - Both modes should only apply light_plus baseline"
echo "   - No AI processing, just basic cleanup"
echo ""
echo "================================================"
echo "Starting Codescribe with debug logging..."
echo ""

# Enable debug logging
export LOG_LEVEL=DEBUG

# Build, then launch the app's executable directly (not via `open`) so LOG_LEVEL
# is inherited and logs stream to this terminal. The Rust `codescribe` binary was
# retired; the app is now produced by scripts/build-app.sh via `make app`.
make app
"$REPO_ROOT/macos/build/Build/Products/Debug/Codescribe.app/Contents/MacOS/Codescribe"
