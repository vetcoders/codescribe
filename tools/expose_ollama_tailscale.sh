#!/bin/bash
# Expose Ollama to Tailscale network

echo "Exposing Ollama on Tailscale network..."
echo ""
echo "Current setup:"
echo "  Dragon Tailscale IP: 100.82.232.70"
echo "  Ollama default port: 11434"
echo ""

# Method 1: Environment variable (requires Ollama restart)
echo "METHOD 1: Set OLLAMA_HOST environment variable"
echo "  launchctl setenv OLLAMA_HOST 0.0.0.0:11434"
echo "  Then restart Ollama app"
echo ""

# Method 2: SSH tunnel from team members
echo "METHOD 2: Team members use SSH tunnel"
echo "  ssh -L 11434:localhost:11434 dragon"
echo "  Then use: http://localhost:11434"
echo ""

# Method 3: Nginx proxy (if installed)
echo "METHOD 3: Nginx reverse proxy"
echo "  Proxy 100.82.232.70:11434 -> 127.0.0.1:11434"
echo ""

# Method 4: Simple socat forward
echo "METHOD 4: Using socat (simple)"
echo "  socat TCP-LISTEN:11434,bind=100.82.232.70,reuseaddr,fork TCP:localhost:11434"
echo ""

# Test current Ollama binding
echo "Testing current Ollama endpoints:"
echo -n "  localhost: "
curl -s http://127.0.0.1:11434/api/tags > /dev/null 2>&1 && echo "OK" || echo "FAIL"
echo -n "  tailscale: "
curl -s http://100.82.232.70:11434/api/tags > /dev/null 2>&1 && echo "OK" || echo "FAIL"
echo ""

# If socat is available, offer to run it
if command -v socat &> /dev/null; then
    echo "socat is available!"
    echo "Run this to expose Ollama on Tailscale:"
    echo "  sudo socat TCP-LISTEN:11434,bind=100.82.232.70,reuseaddr,fork TCP:localhost:11434 &"
else
    echo "socat not installed. Install with: brew install socat"
fi