# Deployment Guide

## Prerequisites

- Rust 1.85+ toolchain (for building from source)
- The `adk-rust` source tree (path dependency in `Cargo.toml` references `../adk-rust/`)

## Single Binary

```bash
cargo build --release
./target/release/adk-gateway --config /path/to/config.json
```

The control panel UI is embedded in the binary — no separate build step required.

---

## Docker

The project includes a multi-stage Dockerfile producing images under 100MB.

### Build

```bash
# Default build (all default features)
docker build -t adk-gateway .

# With optional feature flags
docker build --build-arg FEATURES="browser,postgres,acp" -t adk-gateway .

# Custom Rust version
docker build --build-arg RUST_VERSION=1.85.0 -t adk-gateway .
```

### Build Arguments

| Arg | Default | Description |
|-----|---------|-------------|
| `RUST_VERSION` | `1.85.0` | Rust toolchain version |
| `FEATURES` | `""` | Comma-separated feature flags |
| `BUILD_PROFILE` | `release` | Build profile (`release` or `dev`) |

### Run

```bash
docker run -d \
  --name adk-gateway \
  -p 18789:3000 \
  -v /path/to/config:/etc/adk-gateway \
  -v /path/to/logs:/var/log/adk-gateway \
  -v /path/to/data:/var/lib/adk-gateway \
  -e GOOGLE_API_KEY=your-key \
  -e TELEGRAM_BOT_TOKEN=your-token \
  adk-gateway
```

### Container Details

- **Runtime**: Alpine 3.20 with tini (PID 1 signal forwarding)
- **User**: Non-root `gateway` user
- **Healthcheck**: `curl -f http://localhost:3000/health` every 30s
- **Volumes**: `/etc/adk-gateway` (config), `/var/log/adk-gateway` (logs), `/var/lib/adk-gateway` (data)
- **Default port**: 3000 (inside container)

### Docker Compose

```yaml
services:
  adk-gateway:
    build:
      context: .
      args:
        FEATURES: "postgres"
    ports:
      - "18789:3000"
    volumes:
      - ./config:/etc/adk-gateway:ro
      - gateway-logs:/var/log/adk-gateway
      - gateway-data:/var/lib/adk-gateway
    env_file: .env
    restart: unless-stopped

volumes:
  gateway-logs:
  gateway-data:
```

---

## Systemd (Linux)

The project includes `adk-gateway.service` with `Type=notify` — the gateway signals readiness via `sd_notify(READY=1)` after binding to the port and completing initialization.

### Install

```bash
# Create service user
sudo useradd -r -s /sbin/nologin adk-gateway

# Create directories
sudo mkdir -p /etc/adk-gateway /var/log/adk-gateway /var/lib/adk-gateway
sudo chown adk-gateway:adk-gateway /var/log/adk-gateway /var/lib/adk-gateway

# Install binary
sudo cp target/release/adk-gateway /usr/local/bin/

# Install config
sudo cp gateway.json /etc/adk-gateway/

# Create environment file for secrets
sudo tee /etc/adk-gateway/environment << 'EOF'
GOOGLE_API_KEY=your-key
TELEGRAM_BOT_TOKEN=your-token
EOF
sudo chmod 600 /etc/adk-gateway/environment

# Install and enable service
sudo cp adk-gateway.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now adk-gateway
```

### Service Features

| Feature | Detail |
|---------|--------|
| Readiness | `Type=notify` with `sd_notify(READY=1)` |
| Startup timeout | 90s (generous for cold starts) |
| Shutdown | SIGTERM → drain (30s) → SIGKILL (5s buffer) |
| Auto-restart | On failure, 5s delay, max 5 attempts per 60s |
| Memory limit | 512MB hard / 384MB high |
| File descriptors | 65536 |
| Security | `ProtectSystem=strict`, `NoNewPrivileges`, `PrivateTmp`, restricted syscalls |
| Secrets | Loaded from `/etc/adk-gateway/environment` |

### Commands

```bash
sudo systemctl status adk-gateway
sudo systemctl restart adk-gateway
journalctl -u adk-gateway -f          # follow logs
journalctl -u adk-gateway --since "1h ago"
```

---

## Launchd (macOS)

The project includes `com.zavora.adk-gateway.plist` using the `env.sh` pattern for environment variables.

### Install

```bash
# Install binary
sudo cp target/release/adk-gateway /usr/local/bin/

# Create env.sh with your secrets
cat > ~/.adk-gateway/env.sh << 'EOF'
export GOOGLE_API_KEY="your-key"
export TELEGRAM_BOT_TOKEN="your-token"
export ANTHROPIC_API_KEY="your-key"
EOF
chmod 600 ~/.adk-gateway/env.sh

# Create log directory
mkdir -p ~/.adk-gateway/logs

# Install plist
cp com.zavora.adk-gateway.plist ~/Library/LaunchAgents/

# Load and start
launchctl load ~/Library/LaunchAgents/com.zavora.adk-gateway.plist
```

### How It Works

The plist uses `/bin/bash -c "source ~/.adk-gateway/env.sh && exec adk-gateway ..."` to inject environment variables before starting the gateway. This avoids the macOS limitation where launchd agents don't inherit shell environment.

### Commands

```bash
# Status
launchctl list | grep com.zavora.adk-gateway

# Stop
launchctl unload ~/Library/LaunchAgents/com.zavora.adk-gateway.plist

# Start
launchctl load ~/Library/LaunchAgents/com.zavora.adk-gateway.plist

# Logs
tail -f ~/.adk-gateway/logs/adk-gateway.stdout.log
tail -f ~/.adk-gateway/logs/adk-gateway.stderr.log
```

---

## Zero-Downtime Restart

Send `SIGUSR1` to trigger a graceful restart:

```bash
kill -USR1 $(pgrep adk-gateway)
```

### Drain Phases

1. **Stop accepting** — new messages are rejected
2. **Drain in-flight** — wait for active requests to complete (configurable via `gateway.drainTimeoutSecs`, default 30s)
3. **Exit** — process exits cleanly; systemd/launchd restarts it

Configure drain timeout:

```json5
{ "gateway": { "drainTimeoutSecs": 30 } }
```

Or via environment: `ADK_DRAIN_TIMEOUT_SECS=30`

---

## CI/CD with GitHub Actions

The project uses a self-hosted macOS runner for continuous deployment.

### Workflow (`.github/workflows/deploy.yml`)

```yaml
name: Build & Deploy
on:
  push:
    branches: [main]

jobs:
  build-and-deploy:
    runs-on: [self-hosted, macOS]
    steps:
      - uses: actions/checkout@v4
        with: { path: adk-gateway }

      - uses: actions/checkout@v4
        with:
          repository: zavora-ai/adk-rust
          path: adk-rust
          token: ${{ secrets.GITHUB_TOKEN }}

      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: "1.85.0" }

      - uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            adk-gateway/target
          key: ${{ runner.os }}-cargo-${{ hashFiles('adk-gateway/Cargo.lock') }}

      - run: cargo test --lib
        working-directory: adk-gateway

      - run: cargo build --release
        working-directory: adk-gateway

      - run: ./scripts/deploy-local.sh
        working-directory: adk-gateway
```

### Deploy Script (`scripts/deploy-local.sh`)

The deploy script:
1. Stops the launchd service
2. Copies the release binary to `/usr/local/bin/`
3. Injects environment variables via `launchctl setenv`
4. Starts the service
5. Polls `/health` (up to 15 attempts, 2s apart) to confirm successful startup

---

## Config Encryption

Encrypt sensitive values (API keys, tokens) at rest using AES-256-GCM.

### Setup

```bash
# Generate key and encrypt all sensitive fields in-place
adk-gateway config encrypt
```

This:
1. Generates a 32-byte key file at `~/.adk-gateway/encryption.key`
2. Scans config for sensitive fields (tokens, API keys, passwords)
3. Encrypts values in-place with `enc:` prefix
4. Adds `gateway.encryption.keyFile` to config

### Config

```json5
{
  "gateway": {
    "encryption": {
      "keyFile": "/path/to/encryption.key"
    }
  }
}
```

Encrypted values look like: `"enc:base64encodedciphertext..."`. The gateway decrypts them transparently at startup.

---

## Health Monitoring and Alerting

### Config

```json5
{
  "healthMonitor": {
    "checkIntervalSecs": 60,
    "failureThreshold": 3,
    "alertWebhookUrl": "https://hooks.slack.com/services/...",
    "alertTelegramAdmin": "123456789"
  }
}
```

### Behavior

- Checks channel connectivity, model reachability, and session store health every `checkIntervalSecs`
- After `failureThreshold` consecutive failures, sends alerts via configured channels
- Sends recovery notifications when components return to healthy state

### Endpoints

```bash
# Overall health
curl http://localhost:18789/health

# Per-component breakdown
curl http://localhost:18789/ui/api/health/components

# Health event history
curl http://localhost:18789/ui/api/health/events

# Prometheus metrics
curl http://localhost:18789/metrics
```

---

## Environment Variables

All sensitive values should use `${VAR_NAME}` in config and be set via environment:

```bash
export GOOGLE_API_KEY=your-key
export GEMINI_API_KEY=your-key
export ANTHROPIC_API_KEY=your-key
export OPENAI_API_KEY=your-key
export TELEGRAM_BOT_TOKEN=123:ABC
export SLACK_BOT_TOKEN=xoxb-...
export SLACK_APP_TOKEN=xapp-...
export GATEWAY_AUTH_TOKEN=my-secret
```

### Gateway-Specific Environment

| Variable | Default | Description |
|----------|---------|-------------|
| `ADK_CONFIG_PATH` | `~/.adk-gateway/gateway.json` | Config file path |
| `ADK_GATEWAY_PORT` | `18789` | Listen port (Docker default: 3000) |
| `ADK_DRAIN_TIMEOUT_SECS` | `30` | Graceful shutdown drain timeout |
| `LOG_FORMAT` | `json` | Log format: `json` or `pretty` |
| `RUST_LOG` | `adk_gateway=info` | Log level filter |

---

## Hot-Reload

Edit the config file — changes are detected within 5 seconds and applied without restart. Invalid configs are rejected with a warning log.

Hot-reload applies to:
- Channel configurations
- Agent settings
- Rate limiter / tool approval / stale context settings
- Cron jobs
- Routing rules

Does **not** hot-reload:
- Gateway port/bind (requires restart)
- Session backend type (requires restart)
- Encryption key file path (requires restart)
