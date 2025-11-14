#!/bin/bash

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Test script for dual hotkey modes in VistaScribe
# This script helps verify CTRL vs CTRL+SHIFT functionality

echo "================================================"
echo "VistaScribe Dual Hotkey Mode Test"
echo "================================================"
echo ""
echo "Current configuration:"
echo "- FORMAT_ENABLED: ${FORMAT_ENABLED:-1}"
echo "- FORMAT_STRATEGY: ${FORMAT_STRATEGY:-ollama}"
echo "- OLLAMA_MODEL: ${OLLAMA_MODEL:-gpt-oss:120b}"
echo "- AGENT_NAME: ${AGENT_NAME:-El Niño}"
echo "- MAX_NEW_TOKENS: ${MAX_NEW_TOKENS:-8192}"
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
echo "3. FORMAT_ENABLED=0 (Light Plus Only):"
echo "   - Set FORMAT_ENABLED=0 in .env"
echo "   - Both modes should only apply light_plus baseline"
echo "   - No AI processing, just basic cleanup"
echo ""
echo "================================================"
echo "Starting VistaScribe with debug logging..."
echo ""

# Enable debug logging
export LOG_LEVEL=DEBUG

# Run the application
uv run python -m vistascribe.main
