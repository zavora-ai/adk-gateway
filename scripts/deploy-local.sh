#!/usr/bin/env bash
# =============================================================================
# deploy-local.sh — Deploy adk-gateway binary and restart the launchd service
# =============================================================================
set -euo pipefail

SERVICE_LABEL="com.zavora.adk-gateway"
PLIST_PATH="$HOME/Library/LaunchAgents/${SERVICE_LABEL}.plist"
BINARY_SRC="target/release/adk-gateway"
BINARY_DST="/usr/local/bin/adk-gateway"
HEALTH_URL="http://127.0.0.1:18789/health"
MAX_RETRIES=15
RETRY_INTERVAL=2

echo "=== adk-gateway deployment ==="
echo "Binary source: ${BINARY_SRC}"
echo "Binary destination: ${BINARY_DST}"
echo ""

# --- Verify the built binary exists ---
if [ ! -f "${BINARY_SRC}" ]; then
    echo "ERROR: Release binary not found at ${BINARY_SRC}"
    echo "Run 'cargo build --release' first."
    exit 1
fi

# --- Stop the service ---
echo "Stopping ${SERVICE_LABEL}..."
if launchctl list | grep -q "${SERVICE_LABEL}"; then
    launchctl unload "${PLIST_PATH}" 2>/dev/null || true
    sleep 2
    echo "  Service stopped."
else
    echo "  Service was not running."
fi

# --- Copy the binary ---
echo "Copying binary to ${BINARY_DST}..."
sudo cp "${BINARY_SRC}" "${BINARY_DST}"
sudo chmod +x "${BINARY_DST}"
echo "  Binary installed."

# --- Start the service ---
echo "Starting ${SERVICE_LABEL}..."

# Inject required env vars into launchd (macOS doesn't inherit shell env)
if [ -n "${GOOGLE_API_KEY:-}" ]; then
    launchctl setenv GOOGLE_API_KEY "${GOOGLE_API_KEY}"
fi
if [ -n "${GEMINI_API_KEY:-}" ]; then
    launchctl setenv GEMINI_API_KEY "${GEMINI_API_KEY}"
fi
if [ -n "${TELEGRAM_BOT_TOKEN:-}" ]; then
    launchctl setenv TELEGRAM_BOT_TOKEN "${TELEGRAM_BOT_TOKEN}"
fi
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    launchctl setenv ANTHROPIC_API_KEY "${ANTHROPIC_API_KEY}"
fi
if [ -n "${OPENAI_API_KEY:-}" ]; then
    launchctl setenv OPENAI_API_KEY "${OPENAI_API_KEY}"
fi
# Inject PATH so MCP server commands (npx, uvx, etc.) can be found
launchctl setenv PATH "${PATH}"
launchctl setenv HOME "${HOME}"

launchctl load "${PLIST_PATH}"
echo "  Service started."

# --- Health check ---
echo "Waiting for health check at ${HEALTH_URL}..."
for i in $(seq 1 ${MAX_RETRIES}); do
    if curl -sf "${HEALTH_URL}" > /dev/null 2>&1; then
        echo "  Health check passed! (attempt ${i}/${MAX_RETRIES})"
        echo ""
        echo "=== Deployment successful ==="
        launchctl list | grep "${SERVICE_LABEL}" || true
        exit 0
    fi
    echo "  Attempt ${i}/${MAX_RETRIES} — waiting ${RETRY_INTERVAL}s..."
    sleep "${RETRY_INTERVAL}"
done

echo ""
echo "ERROR: Health check failed after ${MAX_RETRIES} attempts."
echo "Check logs:"
echo "  tail -f ~/.openclaw/logs/adk-gateway.stdout.log"
echo "  tail -f ~/.openclaw/logs/adk-gateway.stderr.log"
exit 1
