# Configuration Reference

Config file: `~/.openclaw/openclaw.json` (JSON5 format)

Override with: `adk-gateway --config /path/to/config.json`

Environment variables: `${VAR_NAME}` patterns are expanded before parsing.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent` | AgentConfig | `{ model: "google/gemini-2.5-flash" }` | Single-agent shorthand |
| `agents` | AgentsConfig | `{}` | Multi-agent setup |
| `gateway` | ServerSettings | `{ port: 18789 }` | Server settings |
| `channels` | ChannelsConfig | `{}` | Channel configurations |
| `routing` | RoutingConfig | `{ bindings: [] }` | Multi-agent routing |
| `session` | SessionConfig | `{ dmScope: "per-channel-peer" }` | Session management |
| `hooks` | HooksConfig | `{ enabled: false }` | Webhook settings |
| `cron` | CronConfig | `{ jobs: [] }` | Cron jobs |
| `memory` | MemoryConfig? | `null` | Knowledge graph memory |
| `rag` | RagConfig? | `null` | RAG pipeline |
| `auth` | AuthConfig? | `null` | Authentication |
| `plugins` | PluginConfig[] | `[]` | Plugins |
| `conventions` | ConventionConfig | `{ enabled: true }` | Convention files |
| `telemetry` | TelemetryConfig | `{ logFormat: "text" }` | Observability |
| `graphWorkflow` | GraphWorkflowConfig? | `null` | DAG-based workflow execution |
| `awp` | AwpConfig | `{ enabled: false }` | AWP protocol (see [AWP Guide](awp-guide.md)) |

## Agent

```json5
{ "agent": { "model": "google/gemini-2.5-flash" } }
// or with fallbacks:
{ "agent": { "model": { "primary": "google/gemini-2.5-flash", "fallbacks": ["anthropic/claude-sonnet-4"] } } }
```

### Cloud Provider

For enterprise deployments using Vertex AI, Azure OpenAI, or AWS Bedrock:

```json5
{
  "agent": {
    "model": { "primary": "google/gemini-2.5-flash" },
    "cloud_provider": {
      "type": "vertex_ai",
      "project_id": "my-gcp-project",
      "location": "us-central1"
    }
  }
}
```

## Gateway Server

```json5
{
  "gateway": {
    "port": 18789,
    "bind": "loopback",  // "loopback" | "lan" | "tailnet" | custom address
    "auth": { "mode": "token", "token": "${GATEWAY_TOKEN}" }
  }
}
```

## Channels

Five channel types are supported:

```json5
{
  "channels": {
    "telegram": {
      "enabled": true,
      "botToken": "${TELEGRAM_BOT_TOKEN}",
      "dmPolicy": "open",       // "open" | "pairing" | "allowlist" | "disabled"
      "allowFrom": ["*"]
    },
    "slack": {
      "enabled": true,
      "botToken": "${SLACK_BOT_TOKEN}",
      "appToken": "${SLACK_APP_TOKEN}"
    },
    "whatsapp": {
      "enabled": true,
      "phoneNumberId": "${WHATSAPP_PHONE_ID}",
      "accessToken": "${WHATSAPP_ACCESS_TOKEN}",
      "verifyToken": "${WHATSAPP_VERIFY_TOKEN}"
    },
    "discord": {
      "enabled": true,
      "botToken": "${DISCORD_BOT_TOKEN}",
      "applicationId": "${DISCORD_APP_ID}"
    },
    "matrix": {
      "enabled": true,
      "homeserverUrl": "https://matrix.example.com",
      "accessToken": "${MATRIX_ACCESS_TOKEN}",
      "userId": "@bot:example.com"
    }
  }
}
```

## Session

```json5
{
  "session": {
    "dmScope": "per-channel-peer",  // "per-peer" | "per-channel-peer" | "per-account-channel-peer"
    "backend": "inmemory",          // "inmemory" | "sqlite" | "postgres" | "redis" | "firestore"
    "connectionString": "sqlite:///path/to/sessions.db",
    "reset": { "mode": "daily", "atHour": 4, "idleMinutes": 120 }
  }
}
```

### Session Backend Types

| Backend | Feature Flag | Connection String Example |
|---------|-------------|--------------------------|
| `inmemory` | (default) | — |
| `sqlite` | (default) | `sqlite:///path/to/sessions.db` or `:memory:` |
| `postgres` | `--features postgres` | `postgres://user:pass@host:5432/dbname` |
| `redis` | `--features redis` | `redis://host:6379` |
| `firestore` | `--features firestore` | Project ID via environment |

## Memory (Knowledge Graph)

```json5
{
  "memory": {
    "backend": "sqlite",  // "inmemory" | "sqlite" | "postgres" | "neo4j" | "sqlrite"
    "connectionString": "sqlite:///path/to/memory.db",
    "embedding": { "provider": "openai", "model": "text-embedding-3-small" }
  }
}
```

## RAG

```json5
{
  "rag": {
    "vectorStore": "inmemory",  // "inmemory" | "qdrant" | "lancedb" | "pgvector" | "surrealdb"
    "embedding": { "provider": "openai" },
    "chunking": "fixed_size",   // "fixed_size" | "markdown" | "recursive"
    "chunkSize": 512,
    "chunkOverlap": 50,
    "watchDirs": ["./documents"],
    "ingestWebhook": true
  }
}
```

## Auth

```json5
{
  "auth": {
    "mode": "token",
    "token": "${AUTH_TOKEN}",
    "roles": [{ "name": "admin", "permissions": ["*"], "scopes": ["*"] }],
    "userMappings": [{ "userId": "user123", "role": "admin" }],
    "audit": { "enabled": true, "sink": "file", "path": "./audit.log" },
    "sso": { "jwksUrl": "https://...", "issuer": "...", "audience": "..." }
  }
}
```

## Cron

```json5
{
  "cron": {
    "jobs": [
      { "id": "daily-report", "schedule": "0 9 * * *", "message": "ask: Generate daily report", "deliverTo": { "channel": "telegram", "target": "@admin" } }
    ]
  }
}
```

## Telemetry

```json5
{
  "telemetry": {
    "logFormat": "json",        // "text" | "json"
    "otelEndpoint": "http://localhost:4317",
    "metricsEnabled": true
  }
}
```
