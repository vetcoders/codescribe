#!/usr/bin/env python3
"""Final test of all fixes"""

import asyncio
import os

from dotenv import load_dotenv

import vistascribe.client as client

load_dotenv()

# Test 1: Check env vars
print("=" * 60)
print("ENVIRONMENT CHECK")
print("=" * 60)
print(f"OLLAMA_MODEL: {os.environ.get('OLLAMA_MODEL')}")
print(f"OLLAMA_HOST: {os.environ.get('OLLAMA_HOST')}")
print(f"MAX_NEW_TOKENS: {os.environ.get('MAX_NEW_TOKENS')}")
print(f"HOLD_MODS: {os.environ.get('HOLD_MODS')}")
print(f"FORMAT_STRATEGY: {os.environ.get('FORMAT_STRATEGY')}")

# Test 2: Client connection
print("\n" + "=" * 60)
print("CLIENT CONNECTION TEST")
print("=" * 60)

status = client.check_server_status()
print(f"Server status: {status}")


# Test 3: Formatting test
async def test_formatting():
    test_text = "to co ja mam na myśli to że jeśli włączymy transkrypcję"

    print("\n" + "=" * 60)
    print("FORMATTING TEST (assistive=False)")
    print("=" * 60)
    result = await client.format_text_http(test_text, assistive=False)
    if result:
        print(f"✓ Formatted ({len(result)} chars): {result[:100]}...")
        # Check for repetitions
        if len(result) > len(test_text) * 3:
            print("⚠️  WARNING: Response too long, might have repetitions!")
    else:
        print("✗ Formatting failed")

    print("\n" + "=" * 60)
    print("ASSISTIVE MODE TEST (assistive=True)")
    print("=" * 60)
    result = await client.format_text_http(test_text, assistive=True)
    if result:
        print(f"✓ Assistive response ({len(result)} chars): {result[:100]}...")
    else:
        print("✗ Assistive mode failed")


if __name__ == "__main__":
    asyncio.run(test_formatting())

print("\n" + "=" * 60)
print("ALL TESTS COMPLETED")
print("=" * 60)
