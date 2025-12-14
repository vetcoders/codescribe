"""
vocabulary.py — High-performance deterministic dictionary enforcement.

Digests a JSONL lexicon containing terms and mispronunciations/synonyms
and compiles them into a single optimized RegEx for O(1) replacement complexity.
"""

from __future__ import annotations

import json
import logging
import os
import re
from pathlib import Path
from typing import Any

from ..path_utils import repo_root

logger = logging.getLogger(__name__)

# Cache tuple: (compiled_regex, lookup_map, mtime_state)
_LEXICON_CACHE: tuple[re.Pattern | None, dict[str, str], dict[Path, float]] | None = None

# Limits to keep prompts/files sane
_MAX_TERM_LEN = 128
_MAX_VARIANT_LEN = 128
_MAX_VARIANTS_PER_TERM = 10


def _asset_roots() -> list[Path]:
    """Return asset directory candidates (custom env var, bundled, development)."""
    roots = []

    # 1. Check environment variable override
    if custom := os.environ.get("CODESCRIBE_ASSETS_DIR"):
        roots.append(Path(custom).expanduser())

    # 2. Check if running from bundled app (../Resources/python/assets)
    try:
        exe_dir = Path(__file__).resolve().parent
        bundled_assets = exe_dir.parent / "assets"
        if bundled_assets.exists():
            roots.append(bundled_assets)
    except Exception:
        pass

    # 3. Development mode paths
    roots.extend(
        [
            repo_root() / "src" / "codescribe" / "assets",
            repo_root() / "assets",
        ]
    )

    return roots


def sanitize_topic(topic: str | None) -> str:
    """Normalize topic to a safe filename fragment."""
    if not topic:
        return "general"
    cleaned = re.sub(r"[^a-zA-Z0-9_.-]+", "-", topic.strip().lower())
    cleaned = cleaned.strip("-._") or "general"
    return cleaned[:48]


def _collect_lexicon_files(topic: str | None = None) -> list[Path]:
    """Return all lexicon .jsonl files (assets + assets/lexicons)."""
    files: set[Path] = set()
    topic_slug = sanitize_topic(topic) if topic else None
    for root in _asset_roots():
        if not root.exists():
            continue
        for candidate in (root, root / "lexicons"):
            if candidate.exists():
                for p in candidate.glob("*.jsonl"):
                    if topic_slug and p.stem != topic_slug:
                        continue
                    files.add(p)
    return sorted(files)


def _snapshot_state(paths: list[Path]) -> dict[Path, float]:
    state: dict[Path, float] = {}
    for p in paths:
        try:
            state[p] = p.stat().st_mtime
        except FileNotFoundError:
            continue
    return state


def _state_changed(prev: dict[Path, float], current: dict[Path, float]) -> bool:
    if prev.keys() != current.keys():
        return True
    for path, mtime in current.items():
        if prev.get(path) != mtime:
            return True
    return False


def _read_jsonl(path: Path) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                if line.strip():
                    try:
                        entries.append(json.loads(line))
                    except json.JSONDecodeError:
                        continue
    except FileNotFoundError:
        return []
    except Exception as e:  # pragma: no cover - best effort only
        logger.error(f"Failed to read lexicon {path}: {e}")
    return entries


def _normalize_entry(entry: dict[str, Any], topic: str | None = None) -> dict[str, Any] | None:
    term = str(entry.get("term") or "").strip()
    if not term:
        return None
    term = term[:_MAX_TERM_LEN]
    variants_raw = entry.get("mispronunciations") or []
    variants: list[str] = []
    for v in variants_raw:
        s = str(v or "").strip()
        if not s:
            continue
        variants.append(s[:_MAX_VARIANT_LEN])
    if not variants:
        return None
    deduped: list[str] = []
    seen: set[str] = set()
    for v in variants:
        key = v.lower()
        if key in seen:
            continue
        seen.add(key)
        deduped.append(v)
        if len(deduped) >= _MAX_VARIANTS_PER_TERM:
            break
    out: dict[str, Any] = {"term": term, "mispronunciations": deduped}
    if topic:
        out["category"] = sanitize_topic(topic)
    return out


def load_lexicon_entries(
    topic: str | None = None, *, limit: int | None = None
) -> list[dict[str, Any]]:
    """Load lexicon entries; optionally filter by topic and limit size."""
    entries: list[dict[str, Any]] = []
    files = _collect_lexicon_files(topic)
    for path in files:
        for entry in _read_jsonl(path):
            normalized = _normalize_entry(entry, topic=entry.get("category") or path.stem)
            if normalized:
                entries.append(normalized)
                if limit and len(entries) >= limit:
                    return entries
    return entries


def _build_regex() -> tuple[re.Pattern | None, dict[str, str], dict[Path, float]]:
    """Compile the lexicon into a single regex and a lookup map."""
    files = _collect_lexicon_files()
    state = _snapshot_state(files)
    data = load_lexicon_entries()
    if not data:
        return None, {}, state

    lookup_map: dict[str, str] = {}
    for entry in data:
        correct_term = entry.get("term")
        if not correct_term:
            continue
        variants = entry.get("mispronunciations", []) or []
        for variant in variants:
            if not variant:
                continue
            key = variant.strip().lower()
            lookup_map[key] = correct_term.strip()

    if not lookup_map:
        return None, {}, state

    sorted_keys = sorted(lookup_map.keys(), key=len, reverse=True)
    escaped_keys = [re.escape(k) for k in sorted_keys]
    pattern_str = r"(?i)\b(" + "|".join(escaped_keys) + r")\b"

    try:
        pattern = re.compile(pattern_str)
        logger.info(f"Compiled vocabulary regex with {len(lookup_map)} rules.")
        return pattern, lookup_map, state
    except re.error as e:
        logger.error(f"Failed to compile vocabulary regex: {e}")
        return None, {}, state


def _ensure_cache() -> tuple[re.Pattern | None, dict[str, str], dict[Path, float]]:
    global _LEXICON_CACHE
    files = _collect_lexicon_files()
    state = _snapshot_state(files)
    if _LEXICON_CACHE is None or _state_changed(_LEXICON_CACHE[2], state):
        _LEXICON_CACHE = _build_regex()
    return _LEXICON_CACHE


def apply_vocabulary_fixes(text: str) -> str:
    """Apply dictionary replacements to the text."""
    cache = _ensure_cache()
    if cache is None:
        return text
    pattern, lookup, _state = cache
    if not pattern:
        return text

    def _replace_match(match: re.Match[str]) -> str:
        original = match.group(0)
        lower_key = original.lower()
        return lookup.get(lower_key, original)

    return pattern.sub(_replace_match, text)


def lexicon_dir() -> Path:
    """Return the preferred lexicon directory (creates if missing)."""
    for root in _asset_roots():
        lex_dir = root / "lexicons"
        try:
            lex_dir.mkdir(parents=True, exist_ok=True)
            return lex_dir
        except Exception:
            continue
    # Fallback to repo assets root
    fallback = _asset_roots()[0]
    fallback.mkdir(parents=True, exist_ok=True)
    return fallback


def lexicon_path_for_topic(topic: str) -> Path:
    return lexicon_dir() / f"{sanitize_topic(topic)}.jsonl"


def append_lexicon_entries(topic: str, entries: list[dict[str, Any]]) -> tuple[int, Path]:
    """Append normalized entries to the topic lexicon, deduplicating existing ones."""
    topic_slug = sanitize_topic(topic)
    target = lexicon_path_for_topic(topic_slug)
    existing_entries = _read_jsonl(target)
    seen: set[tuple[str, tuple[str, ...]]] = set()
    for entry in existing_entries:
        normalized = _normalize_entry(entry, topic=entry.get("category") or topic_slug)
        if not normalized:
            continue
        key = (
            normalized["term"].lower(),
            tuple(m.lower() for m in normalized["mispronunciations"]),
        )
        seen.add(key)

    added = 0
    target.parent.mkdir(parents=True, exist_ok=True)
    with open(target, "a", encoding="utf-8") as f:
        for entry in entries:
            normalized = _normalize_entry(entry, topic=topic_slug)
            if not normalized:
                continue
            key = (
                normalized["term"].lower(),
                tuple(m.lower() for m in normalized["mispronunciations"]),
            )
            if key in seen:
                continue
            seen.add(key)
            f.write(json.dumps(normalized, ensure_ascii=False) + "\n")
            added += 1
    return added, target


def get_soft_lexicon_context(*, max_entries: int = 50, topic: str | None = None) -> str:
    """Return a compact textual context for assistive prompts."""
    entries = load_lexicon_entries(topic, limit=max_entries)
    if not entries:
        return ""
    lines: list[str] = []
    for entry in entries[:max_entries]:
        mis = entry.get("mispronunciations") or []
        if not mis:
            continue
        lines.append(f"{entry.get('term')}: " + ", ".join(mis))
    if not lines:
        return ""
    return "Prefer these canonical terms (misheard -> correct): " + " | ".join(lines)


def reload_lexicon():
    """Force reload of the lexicon (e.g. if file changed)."""
    global _LEXICON_CACHE
    _LEXICON_CACHE = None
