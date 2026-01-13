#!/bin/bash

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

echo "CODESCRIBE PROCESS INSPECTOR"
echo "======================================"

# Check Ollama model
echo "Ollama loaded models:"
curl -s http://127.0.0.1:11434/api/ps 2>/dev/null | jq -r '.models[].name' 2>/dev/null || echo "  None"
echo ""

# Process info
ps aux | grep -E "CodeScribe|codescribe\\.main|codescribe_server" | grep -v grep | while read user pid cpu mem vsz rss tty stat start time cmd rest; do
    echo "======================================"
    # Extract just the command name
    cmd_name=$(basename "$cmd")
    if [[ "$cmd_name" == "python3" || "$cmd_name" == "python" ]]; then
        cmd_name="$cmd $rest"
        cmd_name=$(echo "$cmd_name" | grep -o "[^ ]*\.py" | head -1)
    fi

    echo "Process: $cmd_name"
    echo "PID: $pid | CPU: $cpu% | MEM: $mem%"

    # Calculate RSS in MB
    rss_mb=$(echo "scale=2; $rss/1024" | bc)
    echo "Memory: ${rss_mb}MB RSS"

    # Check ports
    echo "Ports:"
    lsof -p $pid 2>/dev/null | grep LISTEN | awk '{print "  - " $9}' || echo "  None"

    echo ""
done

# Show current .env config
echo "======================================"
echo "Current .env configuration:"
if [[ -f "$REPO_ROOT/.env" ]]; then
    grep -E "OLLAMA_MODEL|FORMAT_STRATEGY|FORMAT_ENABLED|MAX_NEW_TOKENS|WHISPER_VARIANT" "$REPO_ROOT/.env" | sed 's/^/  /'
else
    echo "  .env not found"
fi
