# Configuration Reference

Config file: `~/.openclaw/openclaw.json` (JSON5 format)

Override with: `adk-gateway --config /path/to/config.json`

Environment variables: `${VAR_NAME}` patterns are expanded before parsing.

## Top-Level Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent` | AgentConfig | `{ model: "anthropic/claude-sonnet-4" }` | Single-agent shorthand |
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
| `mcpServers` | McpServerConfig[] | `[]` | MCP server configurations |
| `runner` | GatewayRunnerConfig | `{ maxIterations: 25 }` | Runner iteration limits |
| `rateLimiter` | RateLimitConfig | see below | Tool invocation throttling |
| `toolApproval` | ApprovalConfig | see below | Dangerous tool approval flow |
| `staleContext` | StaleContextConfig | `{ idleThresholdSecs: 14400 }` | Stale context detection |
| `heartbeatV2` | HeartbeatV2Config | see below | Session-integrated heartbeat |
| `multiUser` | MultiUserConfig | see below | Multi-user support |
| `healthMonitor` | HealthMonitorConfig | see below | Periodic health checks + alerting |

---

## Agent

```json5
{ "agent": { "model": "anthropic/claude-sonnet-4" } }
// or with fallbacks:
{ "agent": { "model": { "primary": "anthropic/claude-sonnet-4", "fallbacks": ["google/gemini-2.5-flash"] } } }
```

### Category-Based Model Config

```json5
{
  "agent": {
    "model": {
      "primary": "anthropic/claude-sonnet-4",
      "vision": ["google/gemini-2.5-pro"],
      "code": ["anthropic/claude-sonnet-4"],
      "embedding": ["openai/text-embedding-3-small"],
      "tts": ["google/gemini-2.5-flash"],
      "image_generation": ["google/imagen-4"]
    }
  }
}
```

Categories: `primary`, `vision`, `omni`, `image_generation`, `tts`, `stt`, `code`, `embedding`, `search`, `music`. Each accepts a string or array (fallback chain).

### Cloud Provider

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

---

## Gateway Server

```json5
{
  "gateway": {
    "port": 18789,
    "bind": "loopback",           // "loopback" | "lan" | "tailnet" | custom address
    "auth": { "mode": "token", "token": "${GATEWAY_TOKEN}" },
    "drainTimeoutSecs": 30,       // graceful shutdown drain timeout
    "encryption": {
      "keyFile": "/path/to/encryption.key"
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `port` | u16 | `18789` | HTTP listen port |
| `bind` | string | `"loopback"` | Bind mode: `loopback`, `lan`, `tailnet`, or custom address |
| `auth` | AuthConfig? | `null` | Gateway auth (token/password/none) |
| `drainTimeoutSecs` | u64 | `30` | Seconds to drain in-flight requests on shutdown |
| `encryption` | EncryptionConfig? | `null` | Config value encryption settings |

### Encryption

| Field | Type | Description |
|-------|------|-------------|
| `keyFile` | string (path) | Path to 32-byte AES-256 key file (raw or base64-encoded) |

Generate a key: `adk-gateway config encrypt` (creates key file and encrypts sensitive values in-place). Encrypted values use the `enc:` prefix in the config file.

---

## Runner

Controls the adk-runner iteration budget for gateway-hosted agents.

```json5
{
  "runner": {
    "maxIterations": 25
  }
}
```

| Field | Type | Default | Range | Description |
|-------|------|---------|-------|-------------|
| `maxIterations` | u32 | `25` | [1, 1000] | Max tool-call iterations per request |

Per-agent override: set `maxIterations` on an agent entry (see [Multi-Agent](#multi-agent)) to give specific agents a different budget.

---

## Rate Limiter

Sliding-window rate limiter that prevents runaway tool loops.

```json5
{
  "rateLimiter": {
    "maxCalls": 10,
    "windowSecs": 5,
    "cooldownSecs": 3,
    "maxTriggers": 3
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `maxCalls` | u32 | `10` | Max tool calls allowed within the sliding window |
| `windowSecs` | u64 | `5` | Sliding window duration in seconds |
| `cooldownSecs` | u64 | `3` | Pause duration when rate is exceeded |
| `maxTriggers` | u32 | `3` | Consecutive triggers before request termination |

---

## Tool Approval

Interactive approve/reject flow for dangerous tool calls (Telegram inline buttons).

```json5
{
  "toolApproval": {
    "requireApproval": ["fs_write", "fs_delete", "shell_exec", "run_command"],
    "timeoutSecs": 120
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `requireApproval` | string[] | `[]` (uses built-in defaults) | Tool name patterns requiring approval. Supports globs: `"fs_*"`, `"*_exec"` |
| `timeoutSecs` | u64 | `120` | Seconds before auto-reject |

When `requireApproval` is empty, built-in defaults apply: `fs_write`, `fs_delete`, `shell_exec`, `run_command`.

---

## Stale Context

Welcome-back messages after idle periods with pending task summaries.

```json5
{
  "staleContext": {
    "idleThresholdSecs": 14400
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `idleThresholdSecs` | u64 | `14400` (4 hours) | Seconds of inactivity before session is considered stale |

---

## Health Monitor

Periodic health checks for gateway components with alerting.

```json5
{
  "healthMonitor": {
    "checkIntervalSecs": 60,
    "failureThreshold": 3,
    "alertWebhookUrl": "https://hooks.example.com/alert",
    "alertTelegramAdmin": "123456789"
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `checkIntervalSecs` | u64 | `60` | Seconds between health checks |
| `failureThreshold` | u32 | `3` | Consecutive failures before alerting |
| `alertWebhookUrl` | string? | `null` | Webhook URL for alert delivery (POST JSON) |
| `alertTelegramAdmin` | string? | `null` | Telegram chat ID for alert delivery |

Monitors: channel connectivity, model reachability, session store availability. Sends recovery notifications when components return to healthy state.

---

## Multi-User

Multiple paired users per channel with independent sessions and routing.

```json5
{
  "multiUser": {
    "enabled": true,
    "defaultAgent": "default",
    "routingRules": [
      { "groupId": "group-123", "threadId": "thread-1", "agentId": "research-agent" }
    ],
    "defaultHeartbeatIntervalSecs": 3600
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable multi-user support |
| `defaultAgent` | string | `"default"` | Agent ID for messages without a matching routing rule |
| `routingRules` | GroupRoutingRule[] | `[]` | Per-group/thread agent routing |
| `defaultHeartbeatIntervalSecs` | u64 | `3600` | Default heartbeat interval for new users |

### GroupRoutingRule

| Field | Type | Description |
|-------|------|-------------|
| `groupId` | string | Group ID this rule applies to |
| `threadId` | string? | Optional thread ID for more specific routing |
| `agentId` | string | Agent ID to route messages to |

---

## Heartbeat V2

Session-integrated heartbeat with full conversation context (replaces cron-based heartbeat).

```json5
{
  "heartbeatV2": {
    "enabled": true,
    "defaultIntervalSecs": 3600,
    "prompt": "System heartbeat check: Review the current context..."
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable heartbeat V2 |
| `defaultIntervalSecs` | u64 | `3600` (1 hour) | Default per-user heartbeat interval |
| `prompt` | string | (built-in) | Heartbeat prompt injected into session. Agent replies `HEARTBEAT_OK` if nothing needs attention |

---

## Multi-Agent

```json5
{
  "agents": {
    "defaults": {
      "workspace": "~/.openclaw/workspace",
      "model": "anthropic/claude-sonnet-4",
      "thinkingLevel": "medium"
    },
    "list": [
      {
        "id": "research",
        "model": "google/gemini-2.5-pro",
        "skills": ["web-search"],
        "maxIterations": 50,
        "acp": {
          "endpoint": "http://localhost:8080",
          "timeoutSecs": 300
        }
      }
    ]
  }
}
```

### AgentEntry Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | — | Unique agent identifier |
| `default` | bool | `false` | Whether this is the default agent |
| `workspace` | string? | from defaults | Agent workspace directory |
| `model` | string? | from defaults | Model override |
| `skills` | string[] | `[]` | Skill file names to load |
| `browser` | BrowserConfig? | `null` | Browser/computer-use tool config |
| `tools` | CustomToolConfig[] | `[]` | Custom tool entries |
| `maxIterations` | u32? | `null` | Per-agent iteration limit override [1, 1000] |
| `acp` | object? | `null` | ACP delegation config (requires `acp` feature) |

---

## Channels

Five channel types supported:

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

---

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
| `sqlite` | (default) | `sqlite:///path/to/sessions.db` |
| `postgres` | `--features postgres` | `postgres://user:pass@host:5432/dbname` |
| `redis` | `--features redis` | `redis://host:6379` |
| `firestore` | `--features firestore` | Project ID via environment |

---

## Memory (Knowledge Graph)

```json5
{
  "memory": {
    "backend": "sqlite",  // "inmemory" | "sqlite" | "postgres" | "neo4j"
    "connectionString": "sqlite:///path/to/memory.db",
    "embedding": { "provider": "openai", "model": "text-embedding-3-small" }
  }
}
```

---

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

---

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

---

## Cron

```json5
{
  "cron": {
    "jobs": [
      {
        "id": "daily-report",
        "schedule": "0 9 * * *",
        "message": "ask: Generate daily report",
        "deliverTo": { "channel": "telegram", "target": "@admin" }
      }
    ]
  }
}
```

---

## Telemetry

```json5
{
  "telemetry": {
    "logFormat": "json",           // "text" | "json" | "pretty"
    "otelEndpoint": "http://localhost:4317",
    "metricsEnabled": true,
    "logDir": "/var/log/adk-gateway",
    "logRotation": {
      "rotation": "daily",        // "daily" | "hourly"
      "retentionDays": 7,
      "maxFileSizeMb": 100,
      "format": "json"            // optional override: "json" | "pretty"
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `logFormat` | string | `"text"` | Console log format: `text`, `json`, `pretty` |
| `otelEndpoint` | string? | `null` | OpenTelemetry collector endpoint |
| `metricsEnabled` | bool | `false` | Enable Prometheus metrics at `/metrics` |
| `logDir` | string? | `null` | Directory for persistent rotated log files |
| `logRotation` | LogRotationConfig | see below | Log rotation settings |

### Log Rotation

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `rotation` | string | `"daily"` | Rotation policy: `daily`, `hourly` |
| `retentionDays` | u32 | `7` | Days to retain log files before deletion |
| `maxFileSizeMb` | u64 | `100` | Max file size in MB before size-based rotation |
| `format` | string? | `null` | Override log format for file output |
