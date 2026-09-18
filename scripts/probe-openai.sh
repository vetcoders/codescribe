#!/usr/bin/env bash
# probe-openai.sh — Lokalny probe STT dla OpenAI API
# Użycie:
#   ./probe-openai.sh                             # Domyślny plik + model 'gpt-transcribe'
#   ./probe-openai.sh nagranie.wav                # Własny plik + domyślny model
#   ./probe-openai.sh -m whisper-1 nagranie.wav   # Opt-in do innego modelu (np. whisper-1)
set -euo pipefail

# --- 1. Domyślna konfiguracja ---
DEFAULT_AUDIO="$HOME/.codescribe/last_session.wav"
MODEL="gpt-transcribe"      # Domyślny model zgodnie z życzeniem
STT_LANG="pl"
FORMAT="verbose_json"       # verbose_json / json / text / srt / vtt
AUDIO_FILE=""

# --- 2. Parsowanie flag (opt-in do innego modelu) ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        -m|--model)
            MODEL="$2"
            shift 2
            ;;
        -l|--lang)
            STT_LANG="$2"
            shift 2
            ;;
        -f|--format)
            FORMAT="$2"
            shift 2
            ;;
        -h|--help)
            echo "Użycie: $0 [-m model] [-l lang] [-f format] [plik_audio]"
            echo "Domyślny model: gpt-transcribe"
            exit 0
            ;;
        *)
            AUDIO_FILE="$1"
            shift
            ;;
    esac
done

AUDIO_FILE="${AUDIO_FILE:-$DEFAULT_AUDIO}"

# --- 3. Pobranie klucza LLM_OPENAI_API_KEY z Keychain (z fallbackami) ---
get_openai_key() {
    # a) Zmienna środowiskowa
    if [[ -n "${LLM_OPENAI_API_KEY:-}" ]]; then echo "$LLM_OPENAI_API_KEY"; return 0; fi
    if [[ -n "${OPENAI_API_KEY:-}" ]]; then echo "$OPENAI_API_KEY"; return 0; fi

    # b) macOS Keychain: szukanie po Label/Service/Account
    local k=""
    k="$(security find-generic-password -s "LLM_OPENAI_API_KEY" -w 2>/dev/null || true)"
    [[ -n "$k" ]] && { echo "$k"; return 0; }

    k="$(security find-generic-password -a "LLM_OPENAI_API_KEY" -w 2>/dev/null || true)"
    [[ -n "$k" ]] && { echo "$k"; return 0; }

    k="$(security find-generic-password -s "com.vetcoders.codescribe" -a "LLM_OPENAI_API_KEY" -w 2>/dev/null || true)"
    [[ -n "$k" ]] && { echo "$k"; return 0; }

    k="$(security find-generic-password -s "OPENAI_API_KEY" -w 2>/dev/null || true)"
    [[ -n "$k" ]] && { echo "$k"; return 0; }

    # c) Bundle apki: jeden wpis com.vetcoders.codescribe / codescribe_keychain_bundle_v1
    #    ("b64:" + JSON {version, keys{LLM_OPENAI_API_KEY, ...}}) — tu żyje klucz od 2026-09.
    k="$(security find-generic-password -s "com.vetcoders.codescribe" -a "codescribe_keychain_bundle_v1" -w 2>/dev/null \
        | python3 -c 'import sys,base64,json; r=sys.stdin.read().strip(); r=r[4:] if r.startswith("b64:") else r; print(json.loads(base64.b64decode(r)).get("keys",{}).get("LLM_OPENAI_API_KEY",""))' 2>/dev/null || true)"
    [[ -n "$k" ]] && { echo "$k"; return 0; }

    return 1
}

OPENAI_KEY="$(get_openai_key || true)"
: "${OPENAI_KEY:?Nie znaleziono klucza LLM_OPENAI_API_KEY w Keychain ani w środowisku!}"
(( ${#OPENAI_KEY} > 20 )) || { echo "[ERROR] Klucz OpenAI jest za krótki (${#OPENAI_KEY} znaków)" >&2; exit 1; }

# --- 4. Weryfikacja pliku audio ---
[[ -f "$AUDIO_FILE" ]] || { echo "[ERROR] Brak pliku audio pod ścieżką: $AUDIO_FILE" >&2; exit 1; }
RAW_SIZE_MB=$(du -m "$AUDIO_FILE" | cut -f1)

# Bezpiecznik 25 MB dla OpenAI:
# Jeśli plik przekracza 24 MB, kompresujemy go w locie do MP3 48k mono w /tmp
UPLOAD_FILE="$AUDIO_FILE"
CLEANUP_TMP=""

if (( RAW_SIZE_MB >= 24 )); then
    TMP_MP3="/tmp/probe_openai_$$.mp3"
    echo "[INFO] Plik ma ${RAW_SIZE_MB}MB (przekracza limit 25MB OpenAI)."
    echo "[INFO] Szybka kompresja do MP3 48k mono w locie..."
    ffmpeg -y -hide_banner -loglevel error -i "$AUDIO_FILE" -ar 16000 -ac 1 -b:a 48k "$TMP_MP3"
    UPLOAD_FILE="$TMP_MP3"
    CLEANUP_TMP="$TMP_MP3"
fi

trap '[[ -n "$CLEANUP_TMP" && -f "$CLEANUP_TMP" ]] && rm -f "$CLEANUP_TMP"' EXIT

# --- 5. Wykonanie zapytania ---
echo "============================================================"
echo "=== OpenAI Audio Transcribe Probe                        ==="
echo "Endpoint: https://api.openai.com/v1/audio/transcriptions"
echo "Model:    $MODEL"
echo "Audio:    $AUDIO_FILE ($(du -h "$UPLOAD_FILE" | cut -f1))"
echo "Lang:     $STT_LANG"
echo "Format:   $FORMAT"
echo "Auth:     LLM_OPENAI_API_KEY (${#OPENAI_KEY} znaków)"
echo "============================================================"

START_TS=$(date +%s)

RESP=$(curl --fail-with-body -sS \
    "https://api.openai.com/v1/audio/transcriptions" \
    -H "Authorization: Bearer ${OPENAI_KEY}" \
    -F "file=@${UPLOAD_FILE}" \
    -F "model=${MODEL}" \
    -F "language=${STT_LANG}" \
    -F "response_format=${FORMAT}")

ELAPSED=$(( $(date +%s) - START_TS ))

# --- 6. Prezentacja wyników ---
echo ""
echo "=== Wynik (${ELAPSED}s) ==="
if [[ "$FORMAT" == *"json"* ]]; then
    echo "$RESP" | jq .
else
    echo "$RESP"
fi

unset OPENAI_KEY
