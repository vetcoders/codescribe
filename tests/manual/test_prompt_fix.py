#!/usr/bin/env python3
"""Test if Ollama properly receives system prompt."""

import asyncio
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

os.environ["FORMAT_STRATEGY"] = "ollama"
os.environ["OLLAMA_MODEL"] = "gpt-oss:120b"
os.environ["AGENT_NAME"] = "El Niño"

import vistascribe.llm as llm


async def test_prompt():
    """Test format vs agent mode."""
    print("Testing Ollama System Prompt Fix\n" + "=" * 40)

    # Test 1: Format mode (should just fix text, not explain)
    test1 = "nie wiem czy to przejdzie bo skoro QN30B nie ogarnął"
    print("\nTest 1 - FORMAT mode:")
    print(f"Input: {test1}")
    result1 = await llm.format_text(test1)
    print(f"Output: {result1[:200] if result1 else 'None'}...")

    # Test 2: Agent mode (should be helpful)
    test2 = "El Niño, napisz mi funkcję sortowania"
    print("\nTest 2 - AGENT mode:")
    print(f"Input: {test2}")
    result2 = await llm.format_text(test2)
    print(f"Output: {result2[:200] if result2 else 'None'}...")

    print("\n" + "=" * 40)
    print("If FORMAT mode returns tables/explanations = FAIL")
    print("If FORMAT mode returns just corrected text = PASS")


if __name__ == "__main__":
    asyncio.run(test_prompt())
