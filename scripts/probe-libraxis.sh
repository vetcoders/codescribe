#!/usr/bin/env bash
# probe-libraxis.sh — lokalna sonda /v1/responses na api.libraxis.com
#
# Salwy N równoległych zapytań co INTERVAL sekund, metryki curla (HTTP, TTFT,
# total) i wyciągnięty tekst z SSE do libraxis_probe.jsonl w cwd.
#
# Użycie:
#   ./scripts/probe-libraxis.sh                       # 3 domyślne zapytania w rotacji, model buddy
#   ./scripts/probe-libraxis.sh --prompt "Sformatuj." --resp resp_xxx
#   ./scripts/probe-libraxis.sh --prompt "Co widzisz?" --img https://loctree.com/assets/loctree-logo.png
#   ./scripts/probe-libraxis.sh --model gpt-oss-20b --concurrency 2 --count 1
#   ./scripts/probe-libraxis.sh --batch false --concurrency 3 --count 2   # sekwencyjnie, 3 zapytania × 2 rundy
#
# Parametry:
#   --resp <previous_response_id>   dokleja previous_response_id do każdego zapytania
#   --model <model>                 model (domyślnie: buddy)
#   --img <url>                     input_image (url) doklejony do promptu
#   --prompt <tekst>                jeden prompt dla wszystkich zapytań (zamiast rotacji 1/2/3)
#   --batch true|false              true = salwa równoległa (stagger między strzałami),
#                                   false = zapytania jedno po drugim (domyślnie: true)
#   --concurrency 1..8              zapytań w salwie/rundzie (domyślnie 5)
#   --count 0..10                   liczba salw, 0 = w pętli do ubicia (domyślnie 0)
#   --stagger <ms>                  odstęp między strzałami w salwie (domyślnie 1000)
#   --interval <ms>                 przerwa między salwami (domyślnie 30000)
#   --url <url>                     endpoint (domyślnie https://api.libraxis.com/v1/responses)
#
# Klucz: LBRX_API_KEY_MACIEJ z ~/.zshenv (składnia zsh, więc czytany przez `zsh -c`),
# albo gotowe LIBRAXIS_API_KEY w env. Nigdy nie jest wypisywany.
set -u

# --- Konfiguracja ---
INTERVAL_MS=30000
CONCURRENCY=5
STAGGER_MS=1000
COUNT=0
BATCH=true
LOG_FILE="libraxis_probe.jsonl"
API_URL="${LIBRAXIS_API_URL:-https://api.libraxis.com/v1/responses}"
KEY_FILE="${LIBRAXIS_KEY_FILE:-$HOME/.zshenv}"
MODEL="buddy"
PROMPT=""
IMG=""
RESP=""

usage() {
    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

need_int() { # need_int <nazwa> <wartość> <min> <max>
    [[ "$2" =~ ^[0-9]+$ ]] && (( $2 >= $3 && $2 <= $4 )) \
        || { echo "[ERROR] $1 musi być liczbą całkowitą z zakresu $3..$4 (podano: '$2')" >&2; exit 2; }
}

ms_sleep() { # sleep w milisekundach (macOS sleep przyjmuje ułamki)
    (( $1 > 0 )) && sleep "$(awk -v ms="$1" 'BEGIN { printf "%.3f", ms / 1000 }')"
}

# --- Parsowanie flag ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        --resp)        RESP="$2"; shift 2 ;;
        --model)       MODEL="$2"; shift 2 ;;
        --img)         IMG="$2"; shift 2 ;;
        --prompt)      PROMPT="$2"; shift 2 ;;
        --batch)       BATCH="$2"; shift 2 ;;
        --concurrency) CONCURRENCY="$2"; shift 2 ;;
        --count)       COUNT="$2"; shift 2 ;;
        --stagger)     STAGGER_MS="$2"; shift 2 ;;
        --interval)    INTERVAL_MS="$2"; shift 2 ;;
        --url)         API_URL="$2"; shift 2 ;;
        -h|--help)     usage 0 ;;
        *)             echo "[ERROR] Nieznany parametr: $1" >&2; usage 2 ;;
    esac
done

case "$BATCH" in
    true|false) ;;
    *) echo "[ERROR] --batch przyjmuje true albo false (podano: '$BATCH')" >&2; exit 2 ;;
esac
need_int --concurrency "$CONCURRENCY" 1 8
need_int --count "$COUNT" 0 10
need_int --stagger "$STAGGER_MS" 0 600000
need_int --interval "$INTERVAL_MS" 0 3600000

# --- Klucz: lokalnie, bez echo ---
if [[ -z "${LIBRAXIS_API_KEY:-}" ]]; then
    LIBRAXIS_API_KEY="$(zsh -c "source '$KEY_FILE' 2>/dev/null; printf %s \"\${LBRX_API_KEY_MACIEJ:-}\"")"
fi
(( ${#LIBRAXIS_API_KEY} > 20 )) \
    || { echo "[ERROR] Brak klucza: ustaw LIBRAXIS_API_KEY albo LBRX_API_KEY_MACIEJ w $KEY_FILE" >&2; exit 1; }

# Renderowanie Markdowna w terminalu
render_markdown() {
    local md="$1"
    if command -v glow >/dev/null 2>&1; then
        echo "$md" | glow -
    elif python3 -c "import rich" >/dev/null 2>&1; then
        echo "$md" | python3 -c "import sys, rich.markdown, rich.console; c = rich.console.Console(); c.print(rich.markdown.Markdown(sys.stdin.read()))"
    elif command -v bat >/dev/null 2>&1; then
        echo "$md" | bat -l markdown --style=plain --paging=never
    else
        echo "$md"
    fi
}

cleanup() {
    echo -e "\n[!] Zamykanie probe'a..."
    exit 0
}
trap cleanup SIGINT SIGTERM

# Prompt domyślny wg typu zapytania (rotacja, gdy brak --prompt)
default_prompt() {
    case "$1" in
        1) echo "Co to loctree? Sprawdź w necie. Poza tym co widzisz na obrazku?" ;;
        2) echo "Czym jest Vibecrafting w kontekście inżynierii z agentami AI? Wyjaśnij krótko filary: living tree, dowód w runtime i rzemiosło kodu." ;;
        *) echo "Napisz minimalistyczny przykład w Rust (edycja 2024) implementujący prosty rate limiter token-bucket dla asynchronicznego klienta HTTP. Krótki komentarz i kod." ;;
    esac
}

# Payload budowany przez jq: prompt/img/resp trafiają do JSON z poprawnym escapowaniem
build_payload() {
    local q_idx="$1"
    local text="$PROMPT" img="$IMG"
    if [[ -z "$text" ]]; then
        text="$(default_prompt "$q_idx")"
        # domyślne zapytanie #1 niesie logo loctree, o ile --img nie nadpisuje
        [[ "$q_idx" == "1" && -z "$img" ]] && img="https://loctree.com/assets/loctree-logo.png"
    fi
    jq -n -c \
        --arg model "$MODEL" \
        --arg text "$text" \
        --arg img "$img" \
        --arg resp "$RESP" \
        '{
            model: $model,
            reasoning: { effort: "none" },
            input: [{
                role: "user",
                content: ([{ type: "input_text", text: $text }]
                    + (if $img != "" then [{ type: "input_image", image_url: { url: $img } }] else [] end))
            }],
            stream: true
        } + (if $resp != "" then { previous_response_id: $resp } else {} end)'
}

# Pojedynczy worker odpalany w tle
run_worker() {
    local wave="$1"
    local req_idx="$2"
    local q_idx="$3"
    local out_file="$4"

    local ts_utc
    ts_utc=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    local payload
    payload="$(build_payload "$q_idx")"

    # Lokalny curl, metryki curla w ostatniej linii, stderr razem z body
    local raw_output
    raw_output=$(curl --fail-with-body -sS -N \
        -w "\n__METRICS__:%{http_code}:%{time_total}:%{time_starttransfer}\n" \
        "$API_URL" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${LIBRAXIS_API_KEY}" \
        -d "$payload" 2>&1)

    local metrics_line sse_body http_code time_total time_ttft extracted_text status char_count
    metrics_line=$(echo "$raw_output" | grep -E '^__METRICS__:' | tail -n 1)
    sse_body=$(echo "$raw_output" | grep -v -E '^__METRICS__:')

    if [[ -n "$metrics_line" ]]; then
        http_code=$(echo "$metrics_line" | cut -d: -f2)
        time_total=$(echo "$metrics_line" | cut -d: -f3)
        time_ttft=$(echo "$metrics_line" | cut -d: -f4)
    else
        http_code="ERR"
        time_total="0"
        time_ttft="0"
    fi

    # Eventy SSE jako strumień JSON; `fromjson?` pomija linię, której nie da się sparsować,
    # zamiast wywalać całą ekstrakcję (jq -s padał na jednej złej linii → pusty tekst przy 200).
    local sse_events
    sse_events=$(echo "$sse_body" \
        | awk '/^data: /{d=substr($0,7); if (d!="[DONE]") print d}' \
        | jq -c -R 'fromjson? // empty' 2>/dev/null)

    # Tekst: delty output_text (kształt api.libraxis.com 2026-09-09: response.output_text.delta),
    # a gdy ich brak — output_text z elementu message w response.completed (message nie musi być output[0]).
    extracted_text=$(echo "$sse_events" | jq -r -s '
        (
          ([.[] | select(.type == "response.output_text.delta" or .type == "response.text.delta") | .delta // ""] | join(""))
          | select(length > 0)
        ) // (
          map(select(.type == "response.completed" or .type == "response.done")) | last
          | [.response.output[]? | select(.type == "message") | .content[]? | select(.type == "output_text") | .text] | join("\n")
        ) // empty' 2>/dev/null)

    # response_id z response.completed — do łańcuchowania przez --resp
    local response_id sse_histogram
    response_id=$(echo "$sse_events" | jq -r -s 'map(select(.type == "response.completed" or .type == "response.done")) | last | (.response.id // empty)' 2>/dev/null)
    # histogram typów eventów — jedyny ślad po strumieniu, gdy tekst wyjdzie pusty
    sse_histogram=$(echo "$sse_events" | jq -c -s 'group_by(.type) | map({key: .[0].type, value: length}) | from_entries' 2>/dev/null)
    [[ -n "$sse_histogram" ]] || sse_histogram='{}'
    # response.failed przy HTTP 200 (widziane 2026-09-09: buddy po 4 web_search) — powód do snippetu
    local failed_reason
    failed_reason=$(echo "$sse_events" | jq -r -s 'map(select(.type == "response.failed" or .type == "response.incomplete" or .type == "error")) | last | (.response.error // .error // .response.incomplete_details // empty) | tojson' 2>/dev/null)

    if [[ "$http_code" == "200" && -n "$extracted_text" ]]; then
        status="OK"
        char_count=${#extracted_text}
    else
        status="FAIL"
        char_count=0
    fi

    # Zapis wyniku do pliku tymczasowego JSON
    jq -n -c \
        --arg wave "$wave" \
        --arg req "$req_idx" \
        --arg q_idx "$q_idx" \
        --arg ts "$ts_utc" \
        --arg model "$MODEL" \
        --arg resp_in "$RESP" \
        --arg resp_out "$response_id" \
        --arg status "$status" \
        --arg http "$http_code" \
        --arg ttft "$time_ttft" \
        --arg total "$time_total" \
        --arg chars "$char_count" \
        --arg text "$extracted_text" \
        --argjson events "$sse_histogram" \
        --arg raw_err "${failed_reason:-$(echo "$sse_body" | grep -v -E '^(data: |event: |id: |: heartbeat|$)' | head -n 3; echo "$sse_body" | tail -n 2)}" \
        '{
            wave: ($wave | tonumber),
            req_idx: ($req | tonumber),
            query_idx: ($q_idx | tonumber),
            timestamp: $ts,
            model: $model,
            previous_response_id: (if $resp_in == "" then null else $resp_in end),
            response_id: (if $resp_out == "" then null else $resp_out end),
            status: $status,
            http_code: ($http | tonumber? // $http),
            ttft_sec: ($ttft | tonumber? // 0),
            total_sec: ($total | tonumber? // 0),
            char_count: ($chars | tonumber? // 0),
            sse_events: $events,
            response_text: $text,
            error_snippet: (if $status == "FAIL" then $raw_err else null end)
        }' > "$out_file"
}

# --- Pętla główna ---
echo "=== Start Libraxis Multi-Probe (lokalnie) ==="
echo "Endpoint: ${API_URL} | Model: ${MODEL}${RESP:+ | previous_response_id: ${RESP}}${IMG:+ | img: ${IMG}}"
echo "Prompt: ${PROMPT:-<rotacja domyślnych 1/2/3>}"
echo "Tryb: $([[ "$BATCH" == true ]] && echo "batch (równolegle, stagger ${STAGGER_MS} ms)" || echo "sekwencyjnie (jedno po drugim)")"
echo "Concurrency: ${CONCURRENCY} | Interwał salwy: ${INTERVAL_MS} ms | Salw: ${COUNT} (0 = w pętli do ubicia)"
echo "Log: $(pwd)/${LOG_FILE}"
echo ""

wave=0
queries_distribution=(1 2 3 1 2 3 1 2) # rotacja typów zapytań, gdy brak --prompt

while true; do
    wave=$((wave + 1))
    ts_local=$(date +"%Y-%m-%d %H:%M:%S")
    tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/probe_wave_${wave}_XXXXXX")

    echo "================================================================================"
    echo "[${ts_local}] >>> ODPALENIE SALWY #${wave} (${CONCURRENCY} zapytań, batch=${BATCH}) <<<"

    pids=()
    for i in $(seq 1 "$CONCURRENCY"); do
        q_idx="${queries_distribution[$(( (i - 1) % ${#queries_distribution[@]} ))]}"
        out_file="${tmp_dir}/req_${i}.json"

        echo "  [$(date +'%H:%M:%S')] -> Wystrzelono zapytanie ${i}/${CONCURRENCY} (Query #${q_idx})..."
        if [[ "$BATCH" == true ]]; then
            run_worker "$wave" "$i" "$q_idx" "$out_file" &
            pids+=($!)
            if [[ $i -lt $CONCURRENCY ]]; then
                ms_sleep "$STAGGER_MS"
            fi
        else
            run_worker "$wave" "$i" "$q_idx" "$out_file"
            echo "  [$(date +'%H:%M:%S')]    zapytanie ${i}/${CONCURRENCY} zakończone"
        fi
    done

    if [[ "$BATCH" == true ]]; then
        echo "  [$(date +'%H:%M:%S')] Wszystkie ${CONCURRENCY} zapytań w locie. Czekam na zakończenie..."
        for pid in "${pids[@]}"; do
            wait "$pid"
        done
    fi

    echo ""
    echo "=== WYNIKI SALWY #${wave} ==="

    for i in $(seq 1 "$CONCURRENCY"); do
        res_file="${tmp_dir}/req_${i}.json"
        if [[ ! -f "$res_file" ]]; then
            continue
        fi

        # Dopisanie do głównego logu
        cat "$res_file" >> "$LOG_FILE"

        q_idx=$(jq -r '.query_idx' "$res_file")
        status=$(jq -r '.status' "$res_file")
        http_code=$(jq -r '.http_code' "$res_file")
        ttft=$(jq -r '.ttft_sec' "$res_file")
        total=$(jq -r '.total_sec' "$res_file")
        chars=$(jq -r '.char_count' "$res_file")
        text=$(jq -r '.response_text' "$res_file")
        resp_id=$(jq -r '.response_id // ""' "$res_file")

        echo "--------------------------------------------------------------------------------"
        if [[ "$status" == "OK" ]]; then
            echo -e "Req #${i} [Query #${q_idx}] \033[32m[✓ ${http_code} OK]\033[0m TTFT: ${ttft}s | Total: ${total}s | Znaków: ${chars}${resp_id:+ | id: ${resp_id}}"
            echo ""
            render_markdown "$text"
        else
            err=$(jq -r '.error_snippet' "$res_file")
            echo -e "Req #${i} [Query #${q_idx}] \033[31m[✗ BŁĄD ${http_code}]\033[0m Czas: ${total}s"
            echo "Fragment błędu: $err"
        fi
    done

    rm -rf "$tmp_dir"
    echo "================================================================================"
    if (( COUNT > 0 && wave >= COUNT )); then
        echo "[$(date +'%H:%M:%S')] Salwa #${wave} zakończona. Koniec (--count ${COUNT})."
        break
    fi
    echo "[$(date +'%H:%M:%S')] Salwa #${wave} zakończona. Kolejna za ${INTERVAL_MS} ms..."
    echo ""
    ms_sleep "$INTERVAL_MS"
done
