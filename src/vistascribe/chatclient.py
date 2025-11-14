#!/usr/bin/env python3
"""
Multimodal OpenAI-Compatible Chat Client (no third-party deps)

Features:
- Streaming responses (SSE) using only Python's standard library (urllib).
- Internet search context injection (DuckDuckGo Instant Answer API, stdlib only).
- Multimodal user messages:
  - Text
  - Images: local files (auto-encoded as data URLs) or remote URLs
  - Audio: local files (base64 + inferred format)
- Clean interactive CLI:
  - Commands:
      /image <path-or-url>     attach image to next message (repeatable)
      /audio <path>            attach audio file to next message (repeatable)
      /search <query>          fetch quick context and inject into system message
      /clear                   clear history
      /exit or /quit           leave
  - You can also pass --image/--audio on the command line for initial attachments.

Why no 'requests'?
- This client avoids external dependencies to prevent ModuleNotFoundError: requests.
- Streaming is implemented via reading SSE lines from urllib response.

Quick Start:
  python3 chatclient.py --api-key YOUR_KEY --model gpt-4o-mini
  # With a custom base URL:
  python3 chatclient.py --api-key sk-... --base-url https://api.openai.com/v1 --model gpt-4o
  # Attach a local image and an audio file to your first prompt:
  python3 chatclient.py --api-key sk-... --image ./pic.png --audio ./note.wav

Notes:
- Multimodal content uses OpenAI-compatible message content parts:
  - {"type":"text","text": "..."}
  - {"type":"image_url","image_url":{"url":"<http(s)://... or data URL>"}}
  - {"type":"input_audio","input_audio":{"data":"<base64>","format":"wav|mp3|m4a|ogg|flac|webm"}}
- Not all backends support all modalities; ensure your chosen model supports images/audio.
"""

from __future__ import annotations

import argparse
import base64
import json
import mimetypes
import os
import sys
from collections.abc import Iterator
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import quote_plus, urlparse
from urllib.request import Request, urlopen


# ANSI colors
class Colors:
    USER = "\033[94m"  # Blue
    ASSISTANT = "\033[92m"  # Green
    SYSTEM = "\033[93m"  # Yellow
    RESET = "\033[0m"


def is_url(s: str) -> bool:
    try:
        p = urlparse(s)
        return p.scheme in ("http", "https")
    except Exception:
        return False


def file_to_data_url(path: str) -> str:
    if not os.path.isfile(path):
        raise FileNotFoundError(f"No such file: {path}")
    mime, _ = mimetypes.guess_type(path)
    if not mime:
        # default to binary stream; some UIs accept image/png by default for images
        ext = (os.path.splitext(path)[1] or "").lower().lstrip(".")
        if ext in {"png", "jpg", "jpeg", "gif", "webp"}:
            mime = f"image/{'jpeg' if ext == 'jpg' else ext}"
        else:
            mime = "application/octet-stream"
    with open(path, "rb") as f:
        b64 = base64.b64encode(f.read()).decode("ascii")
    return f"data:{mime};base64,{b64}"


def read_audio_base64(path: str) -> tuple[str, str]:
    """Return (base64_data, format) from a local audio file path."""
    if not os.path.isfile(path):
        raise FileNotFoundError(f"No such file: {path}")
    ext = (os.path.splitext(path)[1] or "").lower().lstrip(".")
    # map common extensions to formats expected by OpenAI-compatible APIs
    fmt_map = {
        "wav": "wav",
        "mp3": "mp3",
        "m4a": "m4a",
        "aac": "m4a",
        "ogg": "ogg",
        "oga": "ogg",
        "flac": "flac",
        "webm": "webm",
        "opus": "ogg",
    }
    fmt = fmt_map.get(ext, ext or "wav")
    with open(path, "rb") as f:
        b64 = base64.b64encode(f.read()).decode("ascii")
    return b64, fmt


def internet_search(query: str, timeout: float = 6.0) -> str:
    """DuckDuckGo Instant Answer API (no HTML). Stdlib only."""
    try:
        url = f"https://api.duckduckgo.com/?q={quote_plus(query)}&format=json&no_html=1&skip_disambig=1"
        req = Request(url, headers={"User-Agent": "MultimodalChatClient/1.0"})
        with urlopen(req, timeout=timeout) as resp:
            if resp.getcode() != 200:
                return f"Search failed: HTTP {resp.getcode()}"
            data = json.loads(resp.read().decode("utf-8", "strict"))
    except (TimeoutError, HTTPError, URLError) as e:
        return f"Search failed: {e}"
    except Exception as e:
        return f"Search failed: {e}"

    result = data.get("AbstractText") or ""
    if not result:
        related = data.get("RelatedTopics") or []
        if isinstance(related, list) and related:
            first = related[0]
            if isinstance(first, dict):
                result = first.get("Text") or ""
    if not result:
        return "No concise result found."
    return (result[:500] + "...") if len(result) > 500 else result


def sse_post(
    url: str, headers: dict[str, str], payload: dict[str, Any], timeout: float = 60.0
) -> Iterator[str]:
    """
    POST JSON and stream Server-Sent Events lines.
    Yields text chunks extracted from choices[0].delta.content when present.
    """
    body = json.dumps(payload).encode("utf-8")
    req_headers = {
        "Content-Type": "application/json",
        "Accept": "text/event-stream",
        "User-Agent": "MultimodalChatClient/1.0",
    }
    req_headers.update(headers or {})
    req = Request(url, data=body, headers=req_headers, method="POST")
    try:
        with urlopen(req, timeout=timeout) as resp:
            status = resp.getcode()
            if status < 200 or status >= 300:
                raise HTTPError(url, status, f"HTTP status {status}", resp.headers, None)
            # Read line-by-line to preserve streaming
            while True:
                line = resp.readline()
                if not line:
                    break
                if line.startswith(b":"):
                    # SSE comment/heartbeat
                    continue
                if not line.strip():
                    continue
                try:
                    decoded = line.decode("utf-8", "replace").strip()
                except Exception:
                    continue
                if not decoded.startswith("data:"):
                    continue
                data_str = decoded[5:].strip()
                if data_str == "[DONE]":
                    break
                try:
                    obj = json.loads(data_str)
                except Exception:
                    continue
                try:
                    delta = obj["choices"][0]["delta"]
                    # delta may have 'content' (str) or other fields
                    content = delta.get("content")
                    if isinstance(content, str) and content:
                        yield content
                except Exception:
                    continue
    except TimeoutError as e:
        raise TimeoutError(f"Request timed out after {timeout}s") from e


def post_once(
    url: str, headers: dict[str, str], payload: dict[str, Any], timeout: float = 60.0
) -> dict[str, Any]:
    """Non-streaming POST that returns parsed JSON using stdlib."""
    body = json.dumps(payload).encode("utf-8")
    req_headers = {
        "Content-Type": "application/json",
        "User-Agent": "MultimodalChatClient/1.0",
    }
    req_headers.update(headers or {})
    req = Request(url, data=body, headers=req_headers, method="POST")
    with urlopen(req, timeout=timeout) as resp:
        status = resp.getcode()
        if status < 200 or status >= 300:
            raise HTTPError(url, status, f"HTTP status {status}", resp.headers, None)
        return json.loads(resp.read().decode("utf-8", "strict"))


def build_user_content(
    text: str | None,
    images: list[str],
    audios: list[str],
) -> list[dict[str, Any]]:
    parts: list[dict[str, Any]] = []
    if text:
        parts.append({"type": "text", "text": text})
    for img in images:
        try:
            url = img if is_url(img) else file_to_data_url(img)
            parts.append({"type": "image_url", "image_url": {"url": url}})
        except Exception as e:
            parts.append({"type": "text", "text": f"[Image attach failed for {img}: {e}]"})
    for a in audios:
        try:
            b64, fmt = read_audio_base64(a)
            parts.append({"type": "input_audio", "input_audio": {"data": b64, "format": fmt}})
        except Exception as e:
            parts.append({"type": "text", "text": f"[Audio attach failed for {a}: {e}]"})
    return parts


def print_system(msg: str) -> None:
    print(f"{Colors.SYSTEM}{msg}{Colors.RESET}")


def print_assistant_prefix() -> None:
    print(f"{Colors.ASSISTANT}Assistant: {Colors.RESET}", end="", flush=True)


def main() -> None:
    default_base = (
        os.environ.get("CHATCLIENT_BASE_URL")
        or os.environ.get("HARMONY_BASE_URL")
        or os.environ.get("LLM_SERVER_URL")
        or ""
    )

    parser = argparse.ArgumentParser(
        description="Multimodal OpenAI-Compatible Chat Client (no external deps)"
    )
    parser.add_argument("--api-key", required=True, help="API key for authentication")
    parser.add_argument(
        "--base-url",
        default=default_base,
        help="Base URL for API (OpenAI-compatible). Required if no env fallback.",
    )
    parser.add_argument(
        "--model", default="gpt-4o-mini", help="Model to use (should support modalities you need)"
    )
    parser.add_argument(
        "--image",
        action="append",
        default=[],
        help="Attach image path or URL to initial user message; repeatable",
    )
    parser.add_argument(
        "--audio",
        action="append",
        default=[],
        help="Attach local audio file to initial user message; repeatable",
    )
    parser.add_argument(
        "--no-stream", action="store_true", help="Disable streaming; receive full response at once"
    )
    parser.add_argument("--timeout", type=float, default=60.0, help="Request timeout seconds")
    args = parser.parse_args()

    base_url = (args.base_url or "").rstrip("/")
    if not base_url:
        parser.error(
            "--base-url is required (or set CHATCLIENT_BASE_URL / HARMONY_BASE_URL / "
            "LLM_SERVER_URL)."
        )

    headers = {
        "Authorization": f"Bearer {args.api_key}",
    }

    messages: list[dict[str, Any]] = []

    print_system("Multimodal Chat Client (OpenAI-Compatible, stdlib only)")
    print_system("Type '/exit' to quit, '/clear' to clear history.")
    print_system("Use commands: /image <path-or-url>, /audio <path>, /search <query>")

    pending_images: list[str] = list(args.image)
    pending_audios: list[str] = list(args.audio)

    while True:
        try:
            user_input = input(f"{Colors.USER}You: {Colors.RESET}").strip()
        except EOFError:
            print()
            break
        except KeyboardInterrupt:
            print_system("Interrupted by user.")
            break

        if not user_input:
            continue

        low = user_input.lower()
        if low in ("/exit", "exit", "quit", "/quit"):
            break
        if low in ("/clear", "clear"):
            messages.clear()
            print_system("History cleared.\n")
            continue

        # Handle inline commands
        if low.startswith("/image "):
            arg = user_input.split(" ", 1)[1].strip()
            if arg:
                pending_images.append(arg)
                print_system(f"[Queued image: {arg}]")
            continue
        if low.startswith("/audio "):
            arg = user_input.split(" ", 1)[1].strip()
            if arg:
                pending_audios.append(arg)
                print_system(f"[Queued audio: {arg}]")
            continue
        if low.startswith("/search "):
            q = user_input.split(" ", 1)[1].strip()
            if q:
                result = internet_search(q)
                messages.append({"role": "system", "content": f"Internet search result: {result}"})
                print_system("[Added search context]")
            continue

        # Optional heuristic: auto-search for specific prompts
        if any(k in low for k in ("search for", "find", "what is", "who is", "current ")):
            result = internet_search(user_input)
            messages.append({"role": "system", "content": f"Internet search result: {result}"})
            print_system("[Added search context]")

        # Build multimodal user message
        content_parts = build_user_content(user_input, pending_images, pending_audios)
        user_message: dict[str, Any]
        if len(content_parts) == 1 and content_parts[0].get("type") == "text":
            # Text-only to maximize compatibility
            user_message = {"role": "user", "content": content_parts[0]["text"]}
        else:
            user_message = {"role": "user", "content": content_parts}

        messages.append(user_message)
        pending_images.clear()
        pending_audios.clear()

        data: dict[str, Any] = {
            "model": args.model,
            "messages": messages,
        }

        if not args.no_stream:
            data["stream"] = True
            print_assistant_prefix()
            full = ""
            try:
                for chunk in sse_post(
                    f"{base_url}/chat/completions", headers, data, timeout=args.timeout
                ):
                    print(chunk, end="", flush=True)
                    full += chunk
                print()
            except Exception as e:
                print()  # ensure newline
                print_system(f"Error: {e}\n")
                continue
            messages.append({"role": "assistant", "content": full})
        else:
            try:
                resp = post_once(
                    f"{base_url}/chat/completions", headers, data, timeout=args.timeout
                )
            except Exception as e:
                print_system(f"Error: {e}\n")
                continue
            try:
                content = resp["choices"][0]["message"]["content"]
            except Exception:
                content = json.dumps(resp)
            print(f"{Colors.ASSISTANT}Assistant: {Colors.RESET}{content}\n")
            messages.append({"role": "assistant", "content": content})


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"{Colors.SYSTEM}Fatal error: {e}{Colors.RESET}", file=sys.stderr)
        sys.exit(1)
