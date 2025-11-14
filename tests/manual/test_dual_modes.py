#!/usr/bin/env python3
"""Test script for dual hotkey modes (CTRL vs CTRL+SHIFT)."""

import asyncio
import os
import sys

# Set test environment
os.environ["LOG_LEVEL"] = "DEBUG"
os.environ["FORMAT_ENABLED"] = "1"
os.environ["FORMAT_STRATEGY"] = "ollama"
os.environ["OLLAMA_MODEL"] = "gpt-oss:120b"

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import logging

import vistascribe.llm as llm

logging.basicConfig(
    level=logging.DEBUG, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
)


async def test_dual_modes():
    """Test both formatting and assistive modes."""
    print("\n" + "=" * 60)
    print("Testing Dual Hotkey Modes")
    print("=" * 60)

    # Test 1: CTRL alone (formatting mode, max 512 tokens)
    test_text_1 = """
    hmm no więc tak jakby chciałem napisać funkcję która sortuje listę
    eee no i właśnie nie wiem czy to przejdzie bo no jakby ten model
    nie jest zbyt dobry w sumie kurwa
    """

    print("\nTest 1: CTRL mode (formatting only, max 512 tokens)")
    print(f"Input: {test_text_1[:100]}...")

    result_1 = await llm.format_text(test_text_1, assistive=False)
    print(f"Output: {result_1[:200] if result_1 else 'None'}...")
    print(f"Output length: {len(result_1) if result_1 else 0} chars")

    # Test 2: CTRL+SHIFT (assistive mode, max 8192 tokens)
    test_text_2 = """
    El Niño, napisz mi funkcję w pythonie która sortuje listę liczb
    używając algorytmu bubble sort
    """

    print("\nTest 2: CTRL+SHIFT mode (assistive AI, max 8192 tokens)")
    print(f"Input: {test_text_2[:100]}...")

    result_2 = await llm.format_text(test_text_2, assistive=True)
    print(f"Output: {result_2[:200] if result_2 else 'None'}...")
    print(f"Output length: {len(result_2) if result_2 else 0} chars")

    # Test 3: FORMAT_ENABLED = False (light_plus only)
    os.environ["FORMAT_ENABLED"] = "0"
    llm.FORMAT_ENABLED = False

    test_text_3 = """
    hmm no więc tak jakby to jest test test test
    eee właśnie właśnie kurwa kurwa no tak
    """

    print("\nTest 3: FORMAT_ENABLED=False (light_plus baseline only)")
    print(f"Input: {test_text_3[:100]}...")

    result_3 = await llm.format_text(test_text_3, assistive=False)
    print(f"Output: {result_3[:200] if result_3 else 'None'}...")
    print(f"Output length: {len(result_3) if result_3 else 0} chars")

    print("\n" + "=" * 60)
    print("Test Summary:")
    print("- CTRL mode: Should return cleaned/formatted text (max 512 chars)")
    print("- CTRL+SHIFT mode: Should return assistive AI response (can be long)")
    print("- FORMAT_ENABLED=False: Should return only light_plus baseline")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(test_dual_modes())
