"""Deterministic Light+ formatting pass.

The Light+ step performs aggressive cleanup on raw Whisper output
without calling any AI model. It should *always* run before we consider
AI enhancement so that Vista and VistaScribe behave identically.
"""

from __future__ import annotations

import re

_FILLERS = re.compile(
    r"\b((?:y+|e+){2,}|hmm+|mhm+|emm+|uh+|umm+|eee+|yyy+)\b[ ,;:!?·…]*", flags=re.IGNORECASE
)
_REPEATED_WORD = re.compile(r"\b(\w+)(?:\s+\1){1,}\b", flags=re.IGNORECASE)
_REPEATED_PHRASE = re.compile(r"((?:\w+\s+){2,3})\1{1,}", flags=re.IGNORECASE)
_REPEATED_PUNCT = re.compile(r"([.!?,;:])\1{1,}")
_PUNCT_SPACING = re.compile(r"\s+([,.;:!?])")
_PUNCT_MERGE = re.compile(r"([,.;:!?])(\S)")
_DASHES = re.compile(r"\s*[–-]\s*")
_MEDICAL_FILLERS = re.compile(
    r"\b(no|tak|właśnie|w sumie|jakby|kurwa|kurde)\b[,\s]*", flags=re.IGNORECASE
)
_ALL_CAPS = re.compile(r"\b([A-ZĄĆĘŁŃÓŚŹŻ]{3,})(\b)")

_POLISH_CORRECTIONS: dict[str, str] = {
    r"\bdziękuje\b": "dziękuję",
    r"\bprosze\b": "proszę",
    r"\bczesc\b": "cześć",
    r"\bmoge\b": "mogę",
    r"\bmusze\b": "muszę",
    r"\bchce\b": "chcę",
    r"\bzle\b": "źle",
    r"\bzolty\b": "żółty",
    r"\bzeby\b": "żeby",
}


def apply_light_plus(text: str) -> str:
    """Return the Light+ cleaned version of *text* (idempotent)."""

    if not text:
        return ""

    t = re.sub(r"\s+", " ", text).strip()
    if not t:
        return ""

    t = _FILLERS.sub("", t).strip()
    if not t:
        return ""

    if t[-1] not in ".!?":
        t += "."

    parts = re.split(r"([.!?]\s+)", t)
    out: list[str] = []
    for idx, part in enumerate(parts):
        if idx == 0 or (idx % 2 == 0 and idx > 0):
            if part:
                part = part[0].upper() + part[1:]
        out.append(part)
    t = "".join(out)

    t = _REPEATED_WORD.sub(r"\1", t)
    t = _REPEATED_PHRASE.sub(r"\1", t)
    t = _REPEATED_PUNCT.sub(r"\1", t)
    t = _PUNCT_SPACING.sub(r"\1", t)
    t = _PUNCT_MERGE.sub(r"\1 \2", t)
    t = _DASHES.sub(" - ", t)
    t = _MEDICAL_FILLERS.sub("", t)
    for pattern, replacement in _POLISH_CORRECTIONS.items():
        t = re.sub(pattern, replacement, t, flags=re.IGNORECASE)

    def _caps_guard(match: re.Match[str]) -> str:
        return match.group(0).capitalize()

    t = _ALL_CAPS.sub(_caps_guard, t)
    return re.sub(r"\s+", " ", t).strip()
