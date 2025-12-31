"""AI formatting helpers (Light+ baseline + optional Harmony/Ollama).

The heavy local MLX-LM path has been removed. CodeScribe now applies the
Light+ deterministic pass *always*, then optionally calls an AI formatter
that speaks the Harmony `/v1/responses` protocol (either via api.libraxis
or api.openai.com) or a local Ollama daemon.
"""

from __future__ import annotations

import logging
import os
from collections.abc import Iterable
from typing import Any, cast

from dotenv import load_dotenv

from .formatting import apply_light_plus
from .formatting.vocabulary import get_soft_lexicon_context
from .path_utils import repo_root, user_data_root
from .settings_store import VistaSettings, get_settings, update_settings

# Optional CodeScribe context injection (assistive mode memory)
try:  # pragma: no cover - available only in dev trays
    from .codescribe_context import inject_context_to_prompt

    HAS_CODESCRIBE = True
except ImportError:  # pragma: no cover
    HAS_CODESCRIBE = False

# Load .env files: repo defaults first, then user overrides
_repo_env = repo_root() / ".env"
if _repo_env.exists():
    load_dotenv(dotenv_path=_repo_env)
else:
    load_dotenv()

# Load user data .env (~/.CodeScribe/.env) - overrides repo settings
_user_env = user_data_root() / ".env"
if _user_env.exists():
    load_dotenv(dotenv_path=_user_env, override=True)

logging.basicConfig(
    level=os.environ.get("LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger("llm")

# --- General prompts ------------------------------------------------------
AGENT_NAME = os.environ.get("AGENT_NAME", "asystent")
AGENT_PROMPT = (
    f"Jesteś '{AGENT_NAME}' – augmentujesz i formatujesz transkrypcje użytkownika. "
    "NIE odpowiadaj jak chatbot. NIE zadawaj pytań. NIE dodawaj komentarzy od siebie. "
    "Słuchaj instrukcji użytkownika (np. 'sformatuj w tabelę', 'zrób listę', 'wypunktuj') "
    "i stosuj je do treści którą otrzymujesz. Zwracaj TYLKO sformatowany/augmentowany tekst. "
    "Możesz używać kaomoji, ale nigdy emoji."
)
FORMAT_PROMPT = (
    "TYLKO popraw błędy, interpunkcję i wielkie litery. Nie wyjaśniaj, nie dodawaj "
    "komentarzy, nie twórz list ani tabel. Zwrot wyłącznie poprawiony tekst."
)

# --- Harmony encoding (token estimates) -----------------------------------
try:  # pragma: no cover - optional but recommended
    from openai_harmony import HarmonyEncodingName, load_harmony_encoding

    _HARMONY_ENCODING = load_harmony_encoding(HarmonyEncodingName.HARMONY_GPT_OSS)
except Exception:  # pragma: no cover - missing native module
    _HARMONY_ENCODING = None


def _count_tokens(text: str) -> int:
    if not text:
        return 0
    if _HARMONY_ENCODING is None:
        return len(text.split())
    try:
        return len(_HARMONY_ENCODING.encode(text, disallowed_special=()))
    except Exception:
        return len(text.split())


# --- Provider plumbing ----------------------------------------------------
try:
    from openai import APIConnectionError, APIError, AsyncOpenAI
except Exception:  # pragma: no cover
    AsyncOpenAI = None  # type: ignore

    class APIConnectionError(Exception):  # type: ignore
        pass

    class APIError(Exception):  # type: ignore
        pass


_harmony_client: AsyncOpenAI | None = None
_harmony_cfg: tuple[str, str] | None = None  # (base_url, api_key)

_ollama_client: AsyncOpenAI | None = None
_ollama_cfg: str | None = None  # base_url

# --- Session tracking for conversation continuity (previous_response_id) ------
_response_ids: dict[str, str] = {}  # "assistive" | "format" | "chat" -> response_id


def get_previous_response_id(session_type: str) -> str | None:
    """Get the last response ID for a session type to continue conversation."""
    return _response_ids.get(session_type)


def set_previous_response_id(session_type: str, response_id: str | None) -> None:
    """Store the response ID for future conversation continuity."""
    if response_id:
        _response_ids[session_type] = response_id
        logger.debug(
            "Stored response_id: %s -> %s",
            session_type,
            response_id[:16] if response_id else "none",
        )
    elif session_type in _response_ids:
        del _response_ids[session_type]


def clear_session(session_type: str | None = None) -> None:
    """Clear stored conversation context. Call when starting fresh."""
    if session_type:
        _response_ids.pop(session_type, None)
    else:
        _response_ids.clear()
    logger.debug("Cleared session: %s", session_type or "all")


def _harmony_base_url() -> str:
    base = os.environ.get("HARMONY_BASE_URL") or os.environ.get("LLM_SERVER_URL")
    if not base:
        raise ValueError(
            "HARMONY_BASE_URL not set. Configure it in .env or the environment "
            "(e.g. HARMONY_BASE_URL=https://api.libraxis.cloud/llm/v1)."
        )
    normalized = base.rstrip("/")
    if normalized.lower().endswith("/responses"):
        normalized = normalized[: -len("/responses")]
    return normalized


def _harmony_api_key() -> str | None:
    for key_name in ("HARMONY_API_KEY", "LIBRAXIS_API_KEY", "OPENAI_API_KEY"):
        token = os.environ.get(key_name)
        if token:
            return token.strip()
    return None


def _get_harmony_client() -> AsyncOpenAI:
    global _harmony_client, _harmony_cfg
    if AsyncOpenAI is None:
        raise RuntimeError("openai python package not available")
    base = _harmony_base_url()
    api_key = _harmony_api_key()
    if not api_key:
        raise RuntimeError("Missing HARMONY_API_KEY/OPENAI_API_KEY")
    cfg = (base, api_key)
    if _harmony_client is None or _harmony_cfg != cfg:
        _harmony_client = AsyncOpenAI(api_key=api_key, base_url=base)
        _harmony_cfg = cfg
    return _harmony_client


def _get_ollama_client() -> AsyncOpenAI:
    """Get AsyncOpenAI client configured for Ollama's /v1/responses endpoint."""
    global _ollama_client, _ollama_cfg
    if AsyncOpenAI is None:
        raise RuntimeError("openai python package not available")
    host = (os.environ.get("OLLAMA_HOST") or "http://127.0.0.1:11434").rstrip("/")
    base = f"{host}/v1"  # Ollama's OpenAI-compatible endpoint
    if _ollama_client is None or _ollama_cfg != base:
        # Ollama doesn't require API key, but OpenAI client needs something
        _ollama_client = AsyncOpenAI(api_key="ollama", base_url=base)
        _ollama_cfg = base
    return _ollama_client


def _extract_response_text(resp: Any) -> str:
    """Extract text from response (ignores response_id)."""
    text, _ = _extract_response_with_id(resp)
    return text


def _extract_response_with_id(resp: Any) -> tuple[str, str | None]:
    """Extract text and response_id from a /v1/responses response object."""
    if resp is None:
        return "", None

    # Get response ID for conversation continuity
    response_id = getattr(resp, "id", None)

    text_chunks: list[str] = []
    output = getattr(resp, "output", None)
    if isinstance(output, Iterable):
        for item in output:
            item_type = getattr(item, "type", None)
            if item_type == "message":
                for content in getattr(item, "content", []) or []:
                    content_type = getattr(content, "type", None)
                    # Handle both "text" and "output_text" types
                    if content_type in ("text", "output_text"):
                        txt = getattr(content, "text", "")
                        if txt:
                            text_chunks.append(txt)
    if text_chunks:
        return "\n".join(text_chunks).strip(), response_id

    text = getattr(resp, "output_text", None)
    if text:
        return str(text).strip(), response_id

    return "", response_id


def _detect_agent_call(text: str) -> bool:
    agent = AGENT_NAME.lower()
    return agent in (text or "").lower()


def _inject_codescribe(text: str, assistive: bool) -> str:
    if not assistive or not HAS_CODESCRIBE:
        return text
    try:  # pragma: no cover - interactive feature
        return inject_context_to_prompt(text)
    except Exception:
        return text


async def _format_with_harmony(text: str, assistive: bool, settings: VistaSettings) -> str | None:
    try:
        client = _get_harmony_client()
    except Exception as exc:
        logger.error(f"Harmony client not ready: {exc}")
        return None

    model = (
        os.environ.get("HARMONY_MODEL") or os.environ.get("OPENAI_FORMAT_MODEL") or "gpt-4o-mini"
    )
    prompt = AGENT_PROMPT if assistive or _detect_agent_call(text) else FORMAT_PROMPT
    lexicon_ctx = get_soft_lexicon_context(max_entries=50) if assistive else ""
    payload = _inject_codescribe(text, assistive)
    max_tokens = settings.ai_assistive_max_tokens if assistive else settings.ai_max_tokens
    system_messages = [{"role": "system", "content": prompt}]
    if lexicon_ctx:
        system_messages.append(
            {
                "role": "system",
                "content": f"Domain lexicon (use these canonical forms): {lexicon_ctx}",
            }
        )
    input_messages: list[dict[str, str]] = system_messages + [
        {"role": "user", "content": payload},
    ]
    try:
        response = await client.responses.create(  # type: ignore[attr-defined]
            model=model,
            input=cast(list[Any], input_messages),
            max_output_tokens=max_tokens,
        )
        out = _extract_response_text(response)
        if out:
            # nosemgrep: python-logger-credential-disclosure - false positive
            logger.debug(
                "Harmony formatting ok (tokens_in=%s, tokens_out≈%s)",
                _count_tokens(payload),
                _count_tokens(out),
            )
            return out
        return None
    except (APIConnectionError, APIError) as exc:
        logger.error(f"Harmony formatting error: {exc}")
        return None


def _ollama_payload(text: str, assistive: bool, settings: VistaSettings) -> dict[str, Any]:
    """Legacy payload builder for /api/generate (fallback for non-responses API)."""
    host = os.environ.get("OLLAMA_HOST") or "http://127.0.0.1:11434"
    model = os.environ.get("OLLAMA_MODEL") or os.environ.get("LLM_ID") or "qwen2.5:3b-instruct"
    temperature = float(os.environ.get("TEMPERATURE", "0.2"))
    top_p = float(os.environ.get("TOP_P", "0.0"))
    prompt = AGENT_PROMPT if assistive else FORMAT_PROMPT
    lexicon_ctx = get_soft_lexicon_context(max_entries=50) if assistive else ""
    if lexicon_ctx:
        prompt = prompt + "\nKontekst słownika: " + lexicon_ctx
    payload = _inject_codescribe(text, assistive)
    max_tokens = settings.ai_assistive_max_tokens if assistive else settings.ai_max_tokens
    return {
        "url": host.rstrip("/") + "/api/generate",
        "json": {
            "model": model,
            "system": prompt,
            "prompt": payload,
            "stream": False,
            "options": {
                "temperature": temperature if assistive else 0.0,
                "top_p": top_p if assistive else 0.0,
                "num_predict": max_tokens,
            },
        },
    }


async def _format_with_ollama(text: str, assistive: bool, settings: VistaSettings) -> str | None:
    """Format text using Ollama's /v1/responses endpoint with conversation continuity."""
    try:
        client = _get_ollama_client()
    except Exception as exc:
        logger.error(f"Ollama client not ready: {exc}")
        return None

    model = os.environ.get("OLLAMA_MODEL") or os.environ.get("LLM_ID") or "qwen2.5:3b-instruct"
    session_type = "assistive" if assistive else "format"

    # Build prompt and messages
    prompt = AGENT_PROMPT if assistive else FORMAT_PROMPT
    lexicon_ctx = get_soft_lexicon_context(max_entries=50) if assistive else ""
    if lexicon_ctx:
        prompt = prompt + "\nKontekst słownika: " + lexicon_ctx
    payload_text = _inject_codescribe(text, assistive)
    max_tokens = settings.ai_assistive_max_tokens if assistive else settings.ai_max_tokens

    # Check for previous conversation context
    prev_response_id = get_previous_response_id(session_type)

    # Build input messages - skip system prompt if continuing conversation
    input_messages: list[dict[str, str]] = []
    if not prev_response_id:
        input_messages.append({"role": "system", "content": prompt})
    input_messages.append({"role": "user", "content": payload_text})

    try:
        # Build kwargs for responses.create
        kwargs: dict[str, Any] = {
            "model": model,
            "input": cast(list[Any], input_messages),
            "max_output_tokens": max_tokens,
        }
        if prev_response_id:
            kwargs["previous_response_id"] = prev_response_id
            logger.debug("Continuing Ollama session: %s -> %s", session_type, prev_response_id[:16])

        response = await client.responses.create(**kwargs)  # type: ignore[attr-defined]
        out, new_response_id = _extract_response_with_id(response)

        # Store response_id for next request
        set_previous_response_id(session_type, new_response_id)

        if out:
            # nosemgrep: python-logger-credential-disclosure - session_type is not a secret
            logger.debug(
                "Ollama formatting ok (session=%s, tokens_in=%s, tokens_out≈%s)",
                session_type,
                _count_tokens(payload_text),
                _count_tokens(out),
            )
            return out
        return None
    except (APIConnectionError, APIError) as exc:
        logger.error(f"Ollama formatting error: {exc}")
        # Clear session on error to start fresh
        set_previous_response_id(session_type, None)
        return None


def _normalize_chat_messages(messages: list[dict[str, str]] | None) -> list[dict[str, str]]:
    normalized: list[dict[str, str]] = []
    if not messages:
        return normalized
    for msg in messages:
        role = (msg.get("role") or "user").strip().lower()
        if role not in {"system", "user", "assistant"}:
            role = "user"
        content = str(msg.get("content") or "").strip()
        if not content:
            continue
        normalized.append({"role": role, "content": content})
    return normalized


async def _chat_with_harmony(messages: list[dict[str, str]], settings: VistaSettings) -> str:
    client = _get_harmony_client()
    model = (
        os.environ.get("HARMONY_CHAT_MODEL")
        or os.environ.get("HARMONY_MODEL")
        or os.environ.get("OPENAI_CHAT_MODEL")
        or "gpt-4o-mini"
    )
    payload = [
        {
            "role": msg["role"],
            "content": msg["content"],
        }
        for msg in messages
    ]
    max_tokens = settings.ai_assistive_max_tokens
    response = await client.responses.create(  # type: ignore[attr-defined]
        model=model,
        input=cast(Any, payload),
        max_output_tokens=max_tokens,
    )
    out = _extract_response_text(response)
    if out:
        return out
    return ""


async def _chat_with_ollama(messages: list[dict[str, str]], settings: VistaSettings) -> str:
    """Chat with Ollama using /v1/responses endpoint with conversation continuity."""
    client = _get_ollama_client()
    model = (
        os.environ.get("OLLAMA_CHAT_MODEL")
        or os.environ.get("OLLAMA_MODEL")
        or os.environ.get("LLM_ID")
        or "qwen2.5:3b-instruct"
    )
    session_type = "chat"
    max_tokens = settings.ai_assistive_max_tokens

    # Check for previous conversation context
    prev_response_id = get_previous_response_id(session_type)

    # Build input messages - skip system if continuing conversation
    input_messages: list[dict[str, str]] = []
    for msg in messages:
        # Skip system messages if we have previous context
        if msg["role"] == "system" and prev_response_id:
            continue
        input_messages.append({"role": msg["role"], "content": msg["content"]})

    # Build kwargs for responses.create
    kwargs: dict[str, Any] = {
        "model": model,
        "input": cast(Any, input_messages),
        "max_output_tokens": max_tokens,
    }
    if prev_response_id:
        kwargs["previous_response_id"] = prev_response_id
        logger.debug("Continuing Ollama chat session: %s", prev_response_id[:16])

    response = await client.responses.create(**kwargs)  # type: ignore[attr-defined]
    out, new_response_id = _extract_response_with_id(response)

    # Store response_id for next request
    set_previous_response_id(session_type, new_response_id)

    if out:
        return out
    return ""


async def run_chat_session(
    messages: list[dict[str, str]] | None,
    *,
    settings: VistaSettings | None = None,
) -> str:
    """Generate a chat response using the configured AI provider."""

    settings = settings or get_settings()
    normalized = _normalize_chat_messages(messages)
    if not normalized:
        return ""

    if normalized[0]["role"] != "system":
        normalized.insert(0, {"role": "system", "content": AGENT_PROMPT})

    provider = settings.ai_provider or "harmony"
    if provider == "ollama":
        try:
            return await _chat_with_ollama(normalized, settings)
        except Exception as exc:
            logger.error(f"Ollama chat failed: {exc}")
            raise
    try:
        return await _chat_with_harmony(normalized, settings)
    except Exception as exc:
        logger.error(f"Harmony chat failed: {exc}")
        raise


async def apply_ai_formatting(text: str, assistive: bool = False) -> str:
    """Apply the optional AI formatter (text is assumed already Light+)."""

    settings = get_settings()
    must_apply = assistive or settings.ai_formatting_enabled
    if not text or not must_apply:
        return text

    provider = settings.ai_provider
    if provider == "ollama":
        maybe_result = _format_with_ollama(text, assistive, settings)
        formatted = await maybe_result
    else:
        formatted = await _format_with_harmony(text, assistive, settings)

    if formatted:
        return formatted
    logger.warning("AI formatting failed for provider=%s; returning baseline", provider)
    return text


async def format_text(raw_text: str, assistive: bool = False) -> str:
    """Full pipeline: Light+ baseline followed by optional AI enhancement."""

    if not raw_text:
        return ""
    baseline = apply_light_plus(raw_text)
    if not baseline:
        return ""
    return await apply_ai_formatting(baseline, assistive=assistive)


def set_ai_formatting_enabled(enabled: bool) -> VistaSettings:
    """Persist the master toggle (tray menu hook)."""
    updated = update_settings({"ai_formatting_enabled": bool(enabled)})
    logger.info("AI formatting %s", "enabled" if updated.ai_formatting_enabled else "disabled")
    return updated


def set_ai_provider(provider: str) -> VistaSettings:
    provider = provider.lower()
    if provider not in {"harmony", "ollama"}:
        raise ValueError("provider must be 'harmony' or 'ollama'")
    updated = update_settings({"ai_provider": provider})
    logger.info("AI provider set to %s", provider)
    return updated


def get_ai_settings() -> VistaSettings:
    return get_settings()


def _ollama_generate(system_prompt: str, text: str, assistive: bool = False) -> str | None:
    """Legacy synchronous helper retained for manual tests.

    Historically tests imported `_ollama_generate` directly; keep a thin wrapper that
    proxies to the current Ollama payload builder so manual e2e checks continue to work.
    """

    import requests  # Local import to avoid cost when unused

    settings = get_settings()
    payload = _ollama_payload(text, assistive, settings)
    if system_prompt:
        payload["json"]["system"] = system_prompt
    try:
        response = requests.post(payload["url"], json=payload["json"], timeout=60)
        response.raise_for_status()
        data = response.json()
        return (data.get("response") or data.get("output") or "").strip() or None
    except Exception as exc:  # pragma: no cover - manual diagnostic helper
        extra = ""
        if isinstance(exc, requests.HTTPError) and getattr(exc, "response", None) is not None:
            try:
                snippet = exc.response.text[:200]
                if snippet:
                    extra = f" body={snippet!r}"
            except Exception as inner_exc:
                logger.debug("Suppressed exception", exc_info=inner_exc)
        logger.error("_ollama_generate failed: %s%s", exc, extra)
        return None
