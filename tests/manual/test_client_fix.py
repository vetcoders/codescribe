#!/usr/bin/env python3
"""Test that client.py can connect to server after healthz fix"""

import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import asyncio

import vistascribe.client as client


def test_sync():
    """Test synchronous functions"""
    print("Testing server detection...")

    # Check server status
    status = client.check_server_status()
    print(f"Server status: {status}")

    if status["server"]:
        print("✓ Server is running!")
        print(f"  - Whisper available: {status['whisper']}")
        print(f"  - LLM available: {status['llm']}")
    else:
        print("✗ Server not detected")

    # Test start_server_if_needed
    print("\nTesting start_server_if_needed...")
    result = client.start_server_if_needed()
    if result:
        print("✓ Server check passed")
    else:
        print("✗ Server check failed")


async def test_async():
    """Test async functions"""
    print("\nTesting transcription (mock)...")

    # Create a simple test WAV file
    import wave

    import numpy as np

    tmp_dir = tempfile.mkdtemp(prefix="vistascribe-manual-")
    test_file = os.path.join(tmp_dir, "test.wav")
    sample_rate = 16000
    duration = 1  # second

    # Generate silence
    samples = np.zeros(int(sample_rate * duration), dtype=np.int16)

    with wave.open(test_file, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(samples.tobytes())

    # Test transcription
    try:
        result = await client.transcribe_http(test_file, language="pl")
        if result:
            print(f"✓ Transcription worked: '{result[:50]}...'")
        else:
            print("✗ Transcription failed")

        # Test formatting
        print("\nTesting formatting...")
        test_text = "test tekstu do formatowania"
        result = await client.format_text_http(test_text, assistive=False)
        if result:
            print(f"✓ Formatting worked: '{result}'")
        else:
            print("✗ Formatting failed")
    finally:
        shutil.rmtree(tmp_dir, ignore_errors=True)


if __name__ == "__main__":
    test_sync()
    asyncio.run(test_async())
