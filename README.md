# adk-gateway

Multi-channel AI gateway for [adk-rust](https://github.com/zavora-ai/adk-rust) agents. Connects Telegram, Slack, WhatsApp, Discord, Matrix, webhooks, and AI agents to your LLM-powered agents via a single binary — with memory, RAG, access control, a React control panel, and [AWP](https://agenticwebprotocol.com) protocol support.

## Quick Start

```bash
# Prerequisites: Rust 1.85+, Node.js 18+

# 1. Clone the repo
git clone https://github.com/zavora-ai/adk-gateway.git
cd adk-gateway

# 2. Build the React UI
cd ui && npm install && npm run build && cd ..

# 3. Start the gateway
cargo run

# 4. Open the control panel
open http://localhost:18789/ui
```

On first run, the UI redirects to a **setup wizard** that walks you through:
1. Choosing a model provider (Free Tier, Frontier, or Auto Intelligence)
2. Setting API keys
3. Connecting a channel (Telegram, Slack, WhatsApp, Discord, or Matrix)

Config is saved to `~/.openclaw/openclaw.json` and hot-reloads on edit.

## Features

- **5 Channels** — Telegram, Slack, WhatsApp, Discord, Matrix
- **14 LLM Providers** — Gemini, Claude, GPT, Ollama, DeepSeek, Groq, OpenRouter, and more
- **Fallback Chains** — Automatic retry across multiple models on failure
- **Multi-Agent** — Create, start, stop, configure specialist agents at runtime
- **Memory** — Per-user knowledge graph with entity extraction and search
- **RAG** — Document ingestion with vector/hybrid/filtered search
- **Graph Workflows** — DAG-based execution with agent nodes, action nodes, conditional routing
- **Access Control** — DM policies, RBAC, role-based tool access, JWT/SSO
- **MCP Integration** — Connect external tool servers via Model Context Protocol
- **AWP Protocol** — Make your gateway discoverable by other AI agents
- **Real-Time UI** — WebSocket-powered control panel with 13 pages
- **Hot-Reload** — Edit config, changes apply without restart
- **Session Backends** — In-memory, SQLite, Postgres, Redis, Firestore

## Control Panel

A React + TypeScript SPA at `/ui` with:

| Page | What it does |
|---|---|
| **Setup Wizard** | First-run guided configuration |
| **Dashboard** | Metrics, channel status, live WebSocket updates |
| **Agent & Model** | Model presets, fallback chains, API keys, cloud providers |
| **Agents** | Multi-agent lifecycle — create, start, stop, configure, delegation permissions |
| **Channels** | Configure all 5 channels with test connection |
| **Sessions** | Active sessions with terminate |
| **AWP** | Health, capabilities, subscriptions, consent |
| **Integrations** | MCP servers, cron jobs, tools |
| **Configuration** | JSON editor with validation |
| **Logs** | Real-time streaming with filters |
| **Memory** | Knowledge graph protocol editor |
| **Settings** | Session backend, memory, RAG config |

## Configuration

```json5
{
  "agent": { "model": "google/gemini-2.5-pro" },
  "channels": {
    "telegram": {
      "botToken": "${TELEGRAM_BOT_TOKEN}",
      "dmPolicy": "open"
    }
  },
  "gateway": { "port": 18789 },
  "memory": {
    "backend": "inmemory",
    "embedding": { "provider": "gemini" }
  }
}
```

See [docs/config-reference.md](docs/config-reference.md) for all options.

## HTTP Endpoints

| Endpoint | Description |
|---|---|
| `GET /health` | Channel health + session count |
| `GET /metrics` | Prometheus-format metrics |
| `GET /ui/*` | React control panel |
| `WS /ws/events` | WebSocket live updates |
| `POST /hooks/inbound` | Webhook for external systems |
| `GET /.well-known/awp.json` | AWP discovery |
| `POST /awp/a2a` | Agent-to-agent messages |

## CLI

```bash
adk-gateway                              # Start (default)
adk-gateway gateway --port 8080          # Custom port
adk-gateway gateway --force              # Kill existing listener
adk-gateway config-validate              # Validate config
adk-gateway config-show                  # Show redacted config
adk-gateway channels-status --probe      # Test channel connectivity
adk-gateway memory search "query" --user-id user1
adk-gateway rag ingest ./documents/
adk-gateway rag search "query" --top-k 10
adk-gateway pairing generate-code
```

## Development

```bash
# Run with verbose logging
cargo run -- -v

# Run tests
cargo test --lib                         # 410+ unit tests
cargo test --test properties             # 30+ property-based tests
cargo test --test wiring_integration     # Integration tests

# UI development (hot reload, proxies to gateway)
cd ui && npm run dev
```

## Deployment

Single binary deployment:

```bash
cargo build --release
./target/release/adk-gateway
```

See [docs/deployment-guide.md](docs/deployment-guide.md) for Docker, systemd, and production setup.

## Current Limitations

| Feature | Status |
|---|---|
| Multi-Agent Codegen | Experimental — generates agent binaries but A2A endpoints are placeholder |
| AWP Commerce | Placeholder — protocol endpoints work, commerce capabilities are declarations only |

## License

Apache-2.0
