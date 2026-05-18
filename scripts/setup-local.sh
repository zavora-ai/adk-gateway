#!/usr/bin/env bash
# =============================================================================
# setup-local.sh — One-time setup for adk-gateway on macOS with launchd
# =============================================================================
# Run this once to:
#   1. Create required directories
#   2. Install the launchd plist
#   3. Print instructions for setting up the GitHub Actions self-hosted runner
# =============================================================================
set -euo pipefail

SERVICE_LABEL="com.zavora.adk-gateway"
PLIST_SRC="$(cd "$(dirname "$0")/.." && pwd)/com.zavora.adk-gateway.plist"
PLIST_DST="$HOME/Library/LaunchAgents/${SERVICE_LABEL}.plist"
OPENCLAW_DIR="$HOME/.openclaw"
LOGS_DIR="${OPENCLAW_DIR}/logs"

echo "=== adk-gateway local setup ==="
echo ""

# --- Create directories ---
echo "Creating directories..."
mkdir -p "${OPENCLAW_DIR}"
mkdir -p "${LOGS_DIR}"
mkdir -p "$HOME/Library/LaunchAgents"
echo "  ${OPENCLAW_DIR}"
echo "  ${LOGS_DIR}"
echo ""

# --- Install launchd plist ---
echo "Installing launchd plist..."
if [ ! -f "${PLIST_SRC}" ]; then
    echo "ERROR: Plist not found at ${PLIST_SRC}"
    exit 1
fi

cp "${PLIST_SRC}" "${PLIST_DST}"
echo "  Installed: ${PLIST_DST}"
echo ""

# --- Verify config exists ---
CONFIG_FILE="${OPENCLAW_DIR}/openclaw.json"
if [ -f "${CONFIG_FILE}" ]; then
    echo "Config found: ${CONFIG_FILE}"
else
    echo "WARNING: Config not found at ${CONFIG_FILE}"
    echo "  Create it before starting the service."
    echo "  See examples/openclaw.json for reference."
fi
echo ""

# --- Verify binary location ---
if [ -f "/usr/local/bin/adk-gateway" ]; then
    echo "Binary found: /usr/local/bin/adk-gateway"
else
    echo "NOTE: Binary not yet installed at /usr/local/bin/adk-gateway"
    echo "  Run 'cargo build --release' then 'scripts/deploy-local.sh' to install."
fi
echo ""

# --- Print runner setup instructions ---
echo "==========================================="
echo "  GitHub Actions Self-Hosted Runner Setup"
echo "==========================================="
echo ""
echo "1. Go to your repo Settings → Actions → Runners → New self-hosted runner"
echo ""
echo "2. Follow GitHub's instructions to download and configure the runner:"
echo "   - Choose macOS as the OS"
echo "   - The runner will be installed in a directory like ~/actions-runner"
echo ""
echo "3. Configure the runner with labels:"
echo "   ./config.sh --url https://github.com/zavora-ai/adk-gateway \\"
echo "     --token <YOUR_TOKEN> \\"
echo "     --labels self-hosted,macOS"
echo ""
echo "4. Install the runner as a launchd service (auto-start on boot):"
echo "   cd ~/actions-runner"
echo "   ./svc.sh install"
echo "   ./svc.sh start"
echo ""
echo "5. Ensure both repos are accessible from the runner's workspace:"
echo "   The workflow checks out adk-gateway and adk-rust as sibling directories."
echo "   No manual repo setup is needed — the workflow handles checkout."
echo ""
echo "6. Ensure /usr/local/bin is writable by the runner user:"
echo "   sudo chown $(whoami) /usr/local/bin"
echo "   (or use sudo in the deploy script)"
echo ""
echo "==========================================="
echo ""
echo "To manually start the service now:"
echo "  launchctl load ${PLIST_DST}"
echo ""
echo "To check status:"
echo "  launchctl list | grep ${SERVICE_LABEL}"
echo ""
echo "To view logs:"
echo "  tail -f ${LOGS_DIR}/adk-gateway.stdout.log"
echo "  tail -f ${LOGS_DIR}/adk-gateway.stderr.log"
echo ""
echo "=== Setup complete ==="
