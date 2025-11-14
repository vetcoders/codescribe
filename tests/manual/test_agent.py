#!/usr/bin/env python3
"""Test agent name detection and mode switching."""

import asyncio
import os
import sys

# Add parent directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Set environment before imports
os.environ["FORMAT_STRATEGY"] = "ollama"
os.environ["OLLAMA_MODEL"] = "qwen3-coder:30b"
os.environ["MAX_NEW_TOKENS"] = "8192"
os.environ["AGENT_NAME"] = "El Niño"

# Import after setting env
import vistascribe.llm as llm


async def test_agent():
    """Test agent name detection."""
    print("Testing Agent Name Detection\n" + "=" * 40)

    agent_name = os.environ.get("AGENT_NAME", "asystent")
    print(f"Agent name: {agent_name}\n")

    test_cases = [
        {
            "text": "napisz mi funkcję która oblicza silnię",
            "mode": "FORMAT",
            "description": "Normal text without agent name",
        },
        {
            "text": f"{agent_name}, napisz mi funkcję która oblicza silnię",
            "mode": "AGENT",
            "description": f"Text with agent name '{agent_name}'",
        },
        {
            "text": f"{agent_name} piszemy prompt dla agenta",
            "mode": "AGENT",
            "description": "Agent called for prompt writing",
        },
        {
            "text": "const my script equals function",
            "mode": "FORMAT",
            "description": "Code dictation without agent",
        },
    ]

    for i, test in enumerate(test_cases, 1):
        print(f"\nTest {i}: {test['description']}")
        print(f"Expected mode: {test['mode']}")
        print(f"Input: {test['text'][:50]}...")

        # Check agent detection
        is_agent = llm._detect_agent_call(test["text"])
        detected_mode = "AGENT" if is_agent else "FORMAT"
        print(f"Detected mode: {detected_mode}")

        if detected_mode != test["mode"]:
            print("❌ FAIL: Mode mismatch!")
        else:
            print("✅ PASS: Mode correctly detected")

        # Test actual formatting (optional - requires Ollama)
        try:
            print("\nTesting LLM response...")
            result = await llm.format_text(test["text"])
            if result:
                print(f"Output preview: {result[:80]}...")
        except Exception as e:
            print(f"LLM test skipped: {e}")

    print("\n" + "=" * 40)
    print("Agent test complete!")


if __name__ == "__main__":
    asyncio.run(test_agent())
