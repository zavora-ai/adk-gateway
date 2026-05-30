# adk-gateway

Multi-channel AI gateway for [adk-rust](https://github.com/zavora-ai/adk-rust) agents. Connects Telegram, Slack, WhatsApp, Discord, Matrix, webhooks, and AI agents to your LLM-powered agents via a single binary — with memory, RAG, access control, a React control panel, and [AWP](https://agenticwebprotocol.com) protocol support.

## Install

```bash
cargo install adk-gateway
```

That's it. The control panel UI is embedded in the binary.

## Run

```bash
adk-gateway
```

Open http://localhost:18789/ui — the setup wizard guides you through configuration on first run.

The wizard walks you through:
1. Choosing a model provider (Free Tier, Frontier, or Auto Intelligence)
2. Setting API keys
3. Connecting a channel (Telegram, Slack, WhatsApp, Discord, or Matrix)

Config is saved to `~/.adk-gateway/gateway.json` and hot-reloads on edit.

## Features

- **5 Channels** — Telegram, Slack, WhatsApp, Discord, Matrix
- **14 LLM Providers** — Gemini, Claude, GPT, Ollama, DeepSeek, Groq, OpenRouter, and more
- **Fallback Chains** — Automatic retry across multiple models on failure
- **Multi-Agent** — Create, start, stop, configure specialist agents at runtime
- **Multi-User** — Multiple paired users per channel with independent sessions
- **Persistent Memory** — SQLite-backed knowledge graph with entity extraction and search (survives restarts)
- **RAG** — Document ingestion with vector/hybrid/filtered search
- **Graph Workflows** — DAG-based execution with agent nodes, action nodes, conditional routing
- **Scheduled Tasks** — Cron-style jobs with agent prompts, delivery targets, suppress keywords, activity logs
- **Heartbeat V2** — Session-integrated heartbeat with full conversation context
- **Tool Approval** — Interactive approve/reject for dangerous tool calls via Telegram inline buttons
- **Stale Context Detection** — Welcome-back messages after idle periods with pending task summaries
- **Rate Limiting** — Sliding-window rate limiter prevents runaway tool loops
- **Health Monitoring** — Periodic health checks with webhook/Telegram alerting
- **Config Encryption** — AES-256-GCM encryption for sensitive config values at rest
- **Zero-Downtime Restart** — SIGUSR1 graceful restart with drain phases
- **ACP Integration** — Delegate tasks to Claude Code or Codex via Agent Communication Protocol
- **MCP Integration** — Connect external tool servers (computer-use, browser, media) via Model Context Protocol
- **108 Agent Tools** — Filesystem, KG, agents, tasks, MCP tools, send_photo, all callable by the LLM
- **Access Control** — DM pairing, RBAC, role-based tool access, JWT/SSO
- **AWP Protocol** — Make your gateway discoverable by other AI agents
- **Real-Time UI** — WebSocket-powered control panel with 14 pages
- **Hot-Reload** — Edit config, changes apply without restart
- **Session Backends** — In-memory, SQLite, Postgres, Redis, Firestore
- **Typing Indicators** — Persistent typing bubble during processing
- **Progress Messages** — Tool execution updates sent to user during long operations
- **Cancel Support** — `/stop` command cancels in-flight requests
- **Image Delivery** — `send_photo` tool sends images directly to Telegram
- **Docker + Systemd** — Production-ready Dockerfile and systemd service

## Control Panel

A React + TypeScript SPA at `/ui` with:

| Page | What it does |
|---|---|
| **Setup Wizard** | First-run guided configuration |
| **Dashboard** | Metrics, channel status, pairing code, live WebSocket updates |
| **Model Providers** | Model presets, fallback chains, API keys, configured providers sidebar |
| **Agents** | Multi-agent lifecycle — create, start, stop, configure, delegation permissions |
| **Channels** | Configure all 5 channels with pairing and test connection |
| **Sessions** | Active sessions with terminate |
| **AWP** | Health, capabilities, subscriptions, consent |
| **Integrations** | MCP servers, tools |
| **Scheduled Tasks** | Create, edit, pause, resume, delete tasks with activity logs and error display |
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
adk-gateway mcp add --json '{"server": {"command": "npx", "args": ["-y", "pkg"]}}'
adk-gateway mcp list
adk-gateway mcp remove <server-id>
adk-gateway config encrypt                # Encrypt sensitive config values in-place
```

## Development

```bash
# From source (requires adk-rust as sibling for path deps during dev)
git clone https://github.com/zavora-ai/adk-rust.git
git clone https://github.com/zavora-ai/adk-gateway.git
cd adk-gateway

# Build UI (for development with hot reload)
cd ui && npm install && npm run dev      # Proxies API to localhost:18789

# Run gateway with verbose logging
cargo run -- -v

# Run tests
cargo test --lib                         # 410+ unit tests
cargo test --test properties             # 30+ property-based tests
cargo test --test wiring_integration     # Integration tests
```

## Deployment

Single binary deployment:

```bash
cargo build --release
./target/release/adk-gateway
```

**Docker:**

```bash
docker build -t adk-gateway .
docker run -v ~/.adk-gateway:/config -p 18789:18789 adk-gateway
```

**Systemd:**

```bash
sudo cp adk-gateway.service /etc/systemd/system/
sudo systemctl enable --now adk-gateway
```

**Launchd (macOS):**

```bash
cp com.zavora.adk-gateway.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.zavora.adk-gateway.plist
```

See [docs/deployment-guide.md](docs/deployment-guide.md) for full production setup.

## Agent Tools

The system agent has access to 108+ tools across these categories:

| Category | Tools |
|----------|-------|
| **Knowledge Graph** | `kg_create_entities`, `kg_add_observations`, `kg_search_nodes`, `kg_read_graph`, `kg_delete_entities` |
| **Agents** | `agent_list`, `agent_create`, `agent_start`, `agent_stop`, `agent_delete`, `agent_configure` |
| **Scheduled Tasks** | `task_list`, `task_create`, `task_cancel`, `task_delete` |
| **Filesystem** | `fs_pwd`, `fs_list`, `fs_tree`, `fs_read`, `fs_search` |
| **Media** | `send_photo` |
| **Computer Use** | 58 tools via MCP (screenshot, click, type, scroll, etc.) |
| **Browser** | 23 tools via MCP (navigate, click, snapshot, evaluate, etc.) |

## Current Limitations

| Feature | Status |
|---|---|
| Multi-Agent Codegen | Experimental — generates agent binaries but A2A endpoints are placeholder |
| AWP Commerce | Placeholder — protocol endpoints work, commerce capabilities are declarations only |

## License

Apache-2.0
