#!/usr/bin/env bash
# probe-libraxis-stt.sh — lokalna sonda STT Libraxis z czasami (WS / NDJSON / REST)
#
# Wysyła PCM 16 kHz z flushami klienta co FLUSH_S sekund (id c1, c2, …, na końcu
# pusty flush c-empty) i sprawdza kontrakt, na którym stoi przypinanie finałów:
#   - dokładnie jeden transcript.final na flush, w kolejności, z echem commit_id;
#   - start_ms/end_ms ciągłe od 0 i równe temu, co wysłano;
#   - pusty flush = pusty finał z duration_ms 0;
#   - words[] w zakresie swojego finału, w kolejności;
#   - REST: timestamp_granularities[]=word zwraca words[].
#
# Użycie:
#   ./scripts/probe-libraxis-stt.sh                         # last_session.wav, wszystkie pasy
#   ./scripts/probe-libraxis-stt.sh nagranie.wav            # własny plik
#   ./scripts/probe-libraxis-stt.sh -t ws --flush-s 5       # tylko WS, flush co 5 s
#   ./scripts/probe-libraxis-stt.sh --seconds 24            # tylko pierwsze 24 s nagrania
#   ./scripts/probe-libraxis-stt.sh --vad -t ws             # segmentacja serwera (VAD)
#
# Parametry:
#   -t|--transport ws|ndjson|rest|all   pas(y) do sprawdzenia (domyślnie all)
#   -l|--lang <kod>                     język (domyślnie pl)
#   --flush-s <s>                       flush co tyle sekund audio (domyślnie 8)
#   --seconds <s>                       przytnij nagranie do s sekund (domyślnie 24, 0 = całość)
#   --base <url>                        host API (domyślnie https://api.libraxis.cloud)
#   --connect-ip <ip>                   NDJSON/REST łączą się z tym IP, TLS/SNI na nazwie hosta
#                                       (WS zawsze po nazwie)
#   --vad                               vad:true — serwer sam commituje na speech.end;
#                                       sprawdza też speech.start/end (audio_ms)
#   -v|--verbose                        wypisz pełne finały (JSON)
#
# Klucz: STT_API_KEY w env, potem Keychain com.vetcoders.codescribe / STT_API_KEY,
# potem LBRX_API_KEY_MACIEJ z ~/.zshenv. Nigdy nie jest wypisywany.
# Exit 0 = kontrakt trzyma na każdym sprawdzonym pasie.
set -euo pipefail

AUDIO_FILE=""
TRANSPORT="all"
STT_LANG="pl"
FLUSH_S=8
SECONDS_MAX=24
BASE="${LIBRAXIS_STT_BASE:-https://api.libraxis.cloud}"
CONNECT_IP=""
VERBOSE=0
VAD=false

usage() { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        -t|--transport) TRANSPORT="$2"; shift 2 ;;
        -l|--lang)      STT_LANG="$2"; shift 2 ;;
        --flush-s)      FLUSH_S="$2"; shift 2 ;;
        --seconds)      SECONDS_MAX="$2"; shift 2 ;;
        --base)         BASE="${2%/}"; shift 2 ;;
        --connect-ip)   CONNECT_IP="$2"; shift 2 ;;
        --vad)          VAD=true; shift ;;
        -v|--verbose)   VERBOSE=1; shift ;;
        -h|--help)      usage 0 ;;
        -*)             echo "[ERROR] Nieznany parametr: $1" >&2; usage 2 ;;
        *)              AUDIO_FILE="$1"; shift ;;
    esac
done
case "$TRANSPORT" in ws|ndjson|rest|all) ;; *) echo "[ERROR] -t: ws|ndjson|rest|all" >&2; exit 2 ;; esac
[[ "$FLUSH_S" =~ ^[0-9]+$ && "$FLUSH_S" -ge 1 ]] || { echo "[ERROR] --flush-s: liczba >= 1" >&2; exit 2; }
[[ "$SECONDS_MAX" =~ ^[0-9]+$ ]] || { echo "[ERROR] --seconds: liczba >= 0" >&2; exit 2; }
for tool in ffmpeg python3 curl; do command -v "$tool" >/dev/null || { echo "[ERROR] brak $tool" >&2; exit 1; }; done
if [[ "$TRANSPORT" == ws || "$TRANSPORT" == all ]]; then
    command -v websocat >/dev/null || { echo "[ERROR] brak websocat (brew install websocat)" >&2; exit 1; }
fi

AUDIO_FILE="${AUDIO_FILE:-$HOME/.codescribe/last_session.wav}"
[[ -f "$AUDIO_FILE" ]] || { echo "[ERROR] Brak pliku audio: $AUDIO_FILE" >&2; exit 1; }

# --- Klucz: lokalnie, bez echo ---
get_stt_key() {
    [[ -n "${STT_API_KEY:-}" ]] && { printf %s "$STT_API_KEY"; return 0; }
    local k
    k="$(security find-generic-password -s com.vetcoders.codescribe -a STT_API_KEY -w 2>/dev/null || true)"
    [[ -n "$k" ]] && { printf %s "$k"; return 0; }
    k="$(zsh -c "source '$HOME/.zshenv' 2>/dev/null; printf %s \"\${LBRX_API_KEY_MACIEJ:-}\"")"
    [[ -n "$k" ]] && { printf %s "$k"; return 0; }
    return 1
}
KEY="$(get_stt_key || true)"
(( ${#KEY} > 20 )) || { echo "[ERROR] Brak klucza STT (STT_API_KEY / Keychain / ~/.zshenv)" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/probe_libraxis_stt_XXXXXX")"
trap 'rm -rf "$WORK"; unset KEY' EXIT

TRIM=()
(( SECONDS_MAX > 0 )) && TRIM=(-t "$SECONDS_MAX")
ffmpeg -hide_banner -loglevel error -y -i "$AUDIO_FILE" "${TRIM[@]}" -ar 16000 -ac 1 -f s16le "$WORK/probe.pcm"
ffmpeg -hide_banner -loglevel error -y -f s16le -ar 16000 -ac 1 -i "$WORK/probe.pcm" "$WORK/probe.wav"

HOST="$(python3 -c 'import sys,urllib.parse as u; print(u.urlparse(sys.argv[1]).hostname)' "$BASE")"
RESOLVE=()
[[ -n "$CONNECT_IP" ]] && RESOLVE=(--resolve "$HOST:443:$CONNECT_IP")

# Wiadomości protokołu + oczekiwane (commit_id, end_ms) per flush
python3 - "$WORK" "$STT_LANG" "$FLUSH_S" "$VAD" <<'PY'
import base64, json, sys
work, lang, flush_s, vad = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4] == "true"
pcm = open(f"{work}/probe.pcm", "rb").read()
step, cut = 6400, flush_s * 32000
msgs = [{"type": "set", "language": lang, "sample_rate": 16000, "encoding": "pcm16",
         "vad": vad, "include_timestamps": True}]
expected, n = [], 0
for start in range(0, len(pcm), cut):
    part = pcm[start:start + cut]
    for off in range(0, len(part), step):
        msgs.append({"type": "chunk", "audio_base64": base64.b64encode(part[off:off + step]).decode()})
    n += 1
    msgs.append({"type": "flush", "id": f"c{n}"})
    expected.append([f"c{n}", round((start + len(part)) / 2 * 1000 / 16000)])
msgs.append({"type": "flush", "id": "c-empty"})
expected.append(["c-empty", expected[-1][1] if expected else 0])
msgs.append({"type": "end"})
with open(f"{work}/msgs.ndjson", "w") as fh:
    fh.writelines(json.dumps(m) + "\n" for m in msgs)
json.dump(expected, open(f"{work}/expected.json", "w"))
PY

echo "============================================================"
echo "=== Libraxis STT Probe (czasy, commit, słowa)            ==="
echo "Host:      $BASE${CONNECT_IP:+ (via $CONNECT_IP)}"
echo "Audio:     $AUDIO_FILE ($(python3 -c "import os,sys; print(f'{os.path.getsize(sys.argv[1])/32000:.1f}s')" "$WORK/probe.pcm") po przycięciu)"
echo "Flush:     co ${FLUSH_S}s, id c1..cN + c-empty | VAD: $VAD | Lang: $STT_LANG | Pasy: $TRANSPORT"
echo "Auth:      STT key (${#KEY} znaków)"
echo "============================================================"

# Wiadomości WS po kolei, z oddechem po flushu (serwer transkrybuje synchronicznie)
ws_feed() {
    while IFS= read -r line; do
        printf '%s\n' "$line"
        case "$line" in *'"flush"'*) sleep 2.5 ;; *'"end"'*) sleep 4 ;; *) sleep 0.02 ;; esac
    done < "$WORK/msgs.ndjson"
}

check() { # check <pas> <plik z eventami ndjson>
    python3 - "$1" "$2" "$WORK/expected.json" "$VERBOSE" "$VAD" <<'PY'
import json, sys
lane, path, expected_path = sys.argv[1], sys.argv[2], sys.argv[3]
verbose, vad = sys.argv[4] == "1", sys.argv[5] == "true"
expected = json.load(open(expected_path))
finals, vad_events, errors = [], [], []
for line in open(path, errors="replace"):
    line = line.strip()
    if not line.startswith("{"):
        continue
    try:
        ev = json.loads(line)
    except json.JSONDecodeError:
        continue
    if ev.get("type") == "error":
        errors.append(f"error event: {ev.get('message')}")
    if ev.get("type") == "transcript.final":
        finals.append(ev)
    if ev.get("type") in ("speech.start", "speech.end"):
        vad_events.append(ev)
# Przy VAD serwer dokłada własne finały (bez commit_id) między flushami klienta.
got = [f.get("commit_id") for f in finals if f.get("commit_id") is not None]
want = [cid for cid, _ in expected]
if got != want:
    errors.append(f"commit ids {got} != {want}")
if not vad and len(finals) != len(expected):
    errors.append(f"{len(finals)} finałów na {len(expected)} flushy")
end_by_id = dict((cid, end_ms) for cid, end_ms in expected)
prev = 0
for f in finals:
    cid = f.get("commit_id") or "auto"
    s, e = f.get("start_ms"), f.get("end_ms")
    words = f.get("words") or []
    flag = "✓"
    want_end = end_by_id.get(cid)
    if (s != prev or e is None or f.get("duration_ms") != (e or 0) - (s or 0)
            or (want_end is not None and abs(e - want_end) > 1)):
        errors.append(f"{cid}: zakres {s}->{e} (dur {f.get('duration_ms')}), oczekiwano start {prev}"
                      + (f", end {want_end}" if want_end is not None else ""))
        flag = "✗"
    if f.get("text") and not words:
        errors.append(f"{cid}: tekst bez words[]"); flag = "✗"
    last = s or 0
    for w in words:
        if not (s <= w["start_ms"] <= w["end_ms"] <= e) or w["start_ms"] < last:
            errors.append(f"{cid}: słowo {w} poza [{s},{e}] albo nie po kolei"); flag = "✗"; break
        last = w["start_ms"]
    print(f"  {flag} {cid:<8} {s:>6}→{e:<6} ms  słów {len(words):>3}  {(f.get('text') or '∅')[:70]}")
    if verbose:
        print("    " + json.dumps(f, ensure_ascii=False))
    prev = e if e is not None else prev
if vad:
    total = expected[-1][1] if expected else 0
    kinds = [ev["type"] for ev in vad_events]
    times = [ev.get("audio_ms") for ev in vad_events]
    if not vad_events:
        errors.append("vad:true, a zero speech.start/end")
    elif None in times:
        errors.append("speech.start/end bez audio_ms")
    else:
        if any(a > b for a, b in zip(times, times[1:])) or not all(0 <= t <= total for t in times):
            errors.append(f"audio_ms nie rośnie albo poza [0,{total}]: {times}")
        if any(k == ("speech.start" if i % 2 else "speech.end") for i, k in enumerate(kinds)):
            errors.append(f"start/end nie na przemian: {kinds}")
    pairs = " ".join(f"{'▶' if ev['type'] == 'speech.start' else '■'}{ev.get('audio_ms')}" for ev in vad_events)
    print(f"  VAD {len(vad_events)} zdarzeń: {pairs}")
for err in errors:
    print(f"  FAIL {lane}: {err}")
print(f"{lane}: {'OK' if not errors else 'FAIL'}")
sys.exit(1 if errors else 0)
PY
}

FAILED=0
if [[ "$TRANSPORT" == ws || "$TRANSPORT" == all ]]; then
    echo ""; echo "=== WS  ${BASE/https:/wss:}/v1/audio/transcribe"
    # websocat nie przypnie IP przy zachowanym SNI, więc WS zawsze idzie po nazwie hosta
    ws_feed | websocat -n -H="Authorization: Bearer ${KEY}" "${BASE/https:/wss:}/v1/audio/transcribe" \
        > "$WORK/ws.out" 2>&1 || true
    # websocat drukuje JSON wieloliniowo tylko gdy serwer tak wyśle; jq -c normalizuje do linii
    jq -c . "$WORK/ws.out" > "$WORK/ws.ndjson" 2>/dev/null || cp "$WORK/ws.out" "$WORK/ws.ndjson"
    check ws "$WORK/ws.ndjson" || FAILED=1
fi

if [[ "$TRANSPORT" == ndjson || "$TRANSPORT" == all ]]; then
    echo ""; echo "=== NDJSON  $BASE/v1/audio/transcribe:stream"
    curl --fail-with-body -sS -m 300 "${RESOLVE[@]}" "$BASE/v1/audio/transcribe:stream" \
        -H "Authorization: Bearer ${KEY}" -H "Content-Type: application/x-ndjson" \
        --data-binary @"$WORK/msgs.ndjson" > "$WORK/nd.ndjson" || true
    check ndjson "$WORK/nd.ndjson" || FAILED=1
fi

if [[ "$TRANSPORT" == rest || "$TRANSPORT" == all ]]; then
    echo ""; echo "=== REST  $BASE/v1/audio/transcriptions (timestamp_granularities[]=word)"
    HTTP=$(curl -sS -m 300 "${RESOLVE[@]}" -o "$WORK/rest.json" -w '%{http_code}' \
        "$BASE/v1/audio/transcriptions" -H "Authorization: Bearer ${KEY}" \
        -F file=@"$WORK/probe.wav" -F language="$STT_LANG" -F response_format=verbose_json \
        -F 'timestamp_granularities[]=word' -F 'timestamp_granularities[]=segment' || echo 000)
    python3 - "$WORK/rest.json" "$HTTP" <<'PY' || FAILED=1
import json, sys
path, http = sys.argv[1], sys.argv[2]
try:
    doc = json.load(open(path))
except Exception:
    doc = {}
words = doc.get("words") or []
dur = float(doc.get("duration") or 0) or 1e9
bad = [w for w in words if not (0 <= w["start"] <= w["end"] <= dur + 0.05)]
order = all(a["start"] <= b["start"] for a, b in zip(words, words[1:]))
ok = http == "200" and words and not bad and order
print(f"  HTTP {http}  segmentów {len(doc.get('segments') or [])}  słów {len(words)}  pierwsze: "
      + ", ".join(f"{w['word']}@{w['start']:.2f}" for w in words[:5]))
print(f"rest: {'OK' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
fi

echo ""
if (( FAILED )); then echo "=== KONTRAKT: FAIL ==="; exit 1; fi
echo "=== KONTRAKT: OK ==="
