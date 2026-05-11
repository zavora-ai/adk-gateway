# Deployment Guide

## Prerequisites

- Rust 1.85+ toolchain
- Node.js 18+ (for building the control panel UI)
- The `adk-rust` source tree (required at build time — path dependencies in `Cargo.toml` reference `../adk-rust/`)

## Single Binary

```bash
# Build the UI first
cd ui && npm install && npm run build && cd ..

# Build the gateway
cargo build --release
./target/release/adk-gateway --config /path/to/config.json
```

## Docker

```dockerfile
FROM rust:1.85 AS builder

# Install Node.js for UI build
RUN curl -fsSL https://deb.nodesource.com/setup_18.x | bash - \
    && apt-get install -y nodejs

WORKDIR /app

# Copy adk-rust source (required for path dependencies)
COPY adk-rust/ /adk-rust/

# Copy gateway source
COPY adk-gateway/ /app/

# Build the React UI
RUN cd ui && npm install && npm run build && cd ..

# Build the gateway binary
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/adk-gateway /usr/local/bin/
COPY --from=builder /app/ui/dist/ /app/ui/dist/
ENTRYPOINT ["adk-gateway"]
```

> **Note:** The Docker build context must include both `adk-rust/` and `adk-gateway/` since Cargo.toml uses path dependencies (`../adk-rust/`). Structure your build context accordingly, or use a parent directory as the Docker context.

```bash
# From the parent directory containing both repos:
docker build -f adk-gateway/Dockerfile -t adk-gateway .

docker run -d \
  -p 18789:18789 \
  -v ~/.openclaw:/root/.openclaw:ro \
  -e GOOGLE_API_KEY=your-key \
  -e TELEGRAM_BOT_TOKEN=your-token \
  adk-gateway
```

## systemd

```ini
# /etc/systemd/system/adk-gateway.service
[Unit]
Description=adk-gateway AI Gateway
After=network.target

[Service]
Type=simple
User=gateway
ExecStart=/usr/local/bin/adk-gateway --config /etc/adk-gateway/config.json
Restart=on-failure
RestartSec=5
Environment=TELEGRAM_BOT_TOKEN=your-token
Environment=GOOGLE_API_KEY=your-key

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable adk-gateway
sudo systemctl start adk-gateway
sudo journalctl -u adk-gateway -f
```

## Environment Variables

All sensitive values should use `${VAR_NAME}` in config and be set via environment:

```bash
export GOOGLE_API_KEY=your-key
export TELEGRAM_BOT_TOKEN=123:ABC
export SLACK_BOT_TOKEN=xoxb-...
export SLACK_APP_TOKEN=xapp-...
export GATEWAY_AUTH_TOKEN=my-secret
```

## Health Monitoring

```bash
# Health check
curl http://localhost:18789/health

# Prometheus metrics
curl http://localhost:18789/metrics

# Channel status
adk-gateway channels-status --probe
```

## Graceful Shutdown

The gateway handles SIGTERM/SIGINT:
1. Stops accepting new messages
2. Waits up to 30 seconds for in-flight messages to complete
3. Closes all channel connections
4. Exits

Use `--force` flag to kill an existing listener on the port before starting.

## Hot-Reload

Edit the config file — changes are detected within 5 seconds and applied without restart. Invalid configs are rejected with a warning log.
