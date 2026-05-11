# Migration Guide: OpenClaw → adk-gateway

## Overview

adk-gateway is a drop-in replacement for OpenClaw with the same JSON5 configuration format. Most configs work without changes.

## Steps

1. **Install adk-gateway**
   ```bash
   cargo install adk-gateway
   ```

2. **Use your existing config** — adk-gateway reads `~/.openclaw/openclaw.json` by default.

3. **Environment variables** — `${VAR_NAME}` expansion works identically.

4. **Start the gateway**
   ```bash
   adk-gateway
   ```

## Config Differences

| Feature | OpenClaw | adk-gateway |
|---------|----------|-------------|
| Config format | JSON5 | JSON5 (same) |
| Default path | `~/.openclaw/openclaw.json` | `~/.openclaw/openclaw.json` (same) |
| Session backends | In-memory only | InMemory, SQLite, Postgres, Redis, Firestore |
| Memory/KG | Not supported | Full knowledge graph with 9 tools |
| RAG | Not supported | Vector store + chunking pipeline |
| Graph workflows | Not supported | adk-graph with action nodes |
| Plugins | Not supported | Lifecycle hook plugins |
| Hot-reload | Not supported | File-watch with validation |
| Metrics | Not supported | Prometheus `/metrics` endpoint |
| Control panel | Not supported | Web UI at `/ui` |

## New Config Sections

These are optional — your existing config works without them:

```json5
{
  // Existing fields work as-is
  "agent": { "model": "anthropic/claude-sonnet-4" },
  "channels": { /* ... */ },

  // New optional sections
  "memory": { "backend": "sqlite", "embedding": { "provider": "openai" } },
  "rag": { "vectorStore": "inmemory", "embedding": { "provider": "openai" } },
  "plugins": [{ "name": "my-plugin", "enabled": true }],
  "telemetry": { "logFormat": "json", "metricsEnabled": true },
  "conventions": { "enabled": true }
}
```

## Breaking Changes

None. adk-gateway is fully backward-compatible with OpenClaw configurations.
