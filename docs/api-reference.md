# API Reference

All endpoints are served on the configured gateway port (default: 18789).

## Core Endpoints

### Health Check

```
GET /health
```

Returns gateway health status with per-component breakdown.

**Response:** `200 OK`
```json
{
  "status": "healthy",
  "uptime_secs": 3600,
  "components": {
    "channels": { "status": "healthy", "details": { "telegram": "connected", "slack": "connected" } },
    "session_store": { "status": "healthy" },
    "model": { "status": "healthy" }
  }
}
```

### Status

```
GET /status
```

Returns detailed gateway status including channel connections and session counts.

### Metrics

```
GET /metrics
```

Prometheus-compatible metrics:

```
adk_gateway_messages_total{channel="telegram",status="success"} 42
adk_gateway_active_sessions 5
adk_gateway_errors_total{channel="telegram"} 2
adk_gateway_tokens_total{model="gpt-4",direction="input"} 1000
```

### Webhooks

```
POST /hooks/inbound
Authorization: Bearer <token>
Content-Type: application/json
```

```json
{
  "text": "Hello, agent!",
  "channel": "telegram",
  "target": "user123",
  "metadata": {}
}
```

**Response (no delivery target):** `{ "status": "ok", "response": "Agent's response" }`

**Response (with delivery target):** `{ "status": "delivered" }`

### WebSocket Events

```
WS /ws/events
```

Real-time event stream:

```json
{"type": "message_received", "channel": "telegram", "user": "user123"}
{"type": "response_sent", "channel": "telegram", "latency_ms": 150}
{"type": "error", "channel": "slack", "error": "connection timeout"}
```

### AWP Discovery

```
GET /.well-known/awp.json
POST /awp/a2a
```

---

## Control Panel UI

```
GET /ui              # Dashboard
GET /ui/sessions     # Active sessions
GET /ui/config       # Configuration editor
GET /ui/logs         # Log viewer
```

---

## Control Panel API

All `/ui/api/*` routes require authentication when auth mode is `token` or `password` (except `/ui/api/login` and `/ui/api/auth/check`).

### Authentication

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/auth/check` | Current auth status |
| POST | `/ui/api/login` | Login (sets session cookie) |
| POST | `/ui/api/logout` | Logout (clears session) |

### Configuration

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/ui/api/config` | Validate and save config to disk |
| GET | `/ui/api/agent` | Current agent model configuration |
| GET | `/ui/api/settings` | Current settings |
| POST | `/ui/api/settings` | Save settings |

### Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/settings/session-status` | Session backend type, health, masked connection string |
| POST | `/ui/api/sessions/{id}/terminate` | Terminate a session |

### Channels

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/channels` | Channel config (tokens masked) |
| POST | `/ui/api/channels` | Save channel config |
| POST | `/ui/api/channels/telegram/probe` | Test Telegram bot token connectivity |

### Agents

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/delegation` | List delegation permissions |
| POST | `/ui/api/delegation` | Add delegation permission (with cycle detection) |
| DELETE | `/ui/api/delegation` | Remove delegation permission |

### Integrations

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/integrations/mcp` | MCP server status |
| POST | `/ui/api/integrations/mcp` | Add/update MCP server |
| DELETE | `/ui/api/integrations/mcp/{id}` | Remove MCP server |
| POST | `/ui/api/integrations/mcp/{id}/toggle` | Enable/disable MCP server |
| GET | `/ui/api/integrations/tools` | Registered tools from tool registry |
| GET | `/ui/api/integrations/cron` | Cron job list |

### Scheduled Tasks

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/ui/api/scheduled-tasks` | Create a scheduled task |
| POST | `/ui/api/scheduled-tasks/{id}/cancel` | Cancel a task |
| POST | `/ui/api/scheduled-tasks/{id}/resume` | Resume a cancelled task |
| DELETE | `/ui/api/scheduled-tasks/{id}` | Delete a task |
| GET | `/ui/api/scheduled-tasks/{id}/logs` | Task activity logs |

### Memory

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/memory/entities` | List all KG entities |

### AWP

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/awp` | AWP summary (health, capabilities, subscriptions) |
| GET | `/ui/api/awp/health` | AWP health state |
| GET | `/ui/api/awp/capabilities` | Capabilities from business context |
| GET | `/ui/api/awp/subscriptions` | Event subscriptions |
| DELETE | `/ui/api/awp/subscriptions/{id}` | Remove subscription |
| GET | `/ui/api/awp/consent` | Consent records |

### Logs

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/logs` | Recent log entries (JSON) |

---

## Phase 2 API Endpoints

### Tool Approval

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/approvals/pending` | List pending tool approval requests |
| GET | `/ui/api/approvals/history` | Approval history entries |
| POST | `/ui/api/approvals/{id}/approve` | Approve a pending tool request |
| POST | `/ui/api/approvals/{id}/reject` | Reject a pending tool request |
| GET | `/ui/api/approvals/config` | Get tool approval configuration |
| POST | `/ui/api/approvals/config` | Save tool approval configuration |

**Pending approval response:**
```json
[
  {
    "id": "req-123",
    "tool": "fs_write",
    "args": { "path": "/tmp/file.txt", "content": "..." },
    "user": "user123",
    "timestamp": "2026-05-16T10:00:00Z",
    "status": "pending"
  }
]
```

### Stale Context

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/settings/stale-context` | Get stale context configuration |
| POST | `/ui/api/settings/stale-context` | Save stale context configuration |

**Request/Response body:**
```json
{ "idleThresholdSecs": 14400 }
```

### Rate Limiter

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/settings/rate-limit` | Get rate limiter configuration |
| POST | `/ui/api/settings/rate-limit` | Save rate limiter configuration |
| GET | `/ui/api/settings/rate-limit/metrics` | Rate limit trigger metrics |

**Request/Response body:**
```json
{
  "maxCalls": 10,
  "windowSecs": 5,
  "cooldownSecs": 3,
  "maxTriggers": 3
}
```

### Health Monitor

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/health/components` | Per-component health status |
| GET | `/ui/api/health/events` | Health event history (failures, recoveries) |

**Components response:**
```json
[
  { "name": "telegram", "status": "healthy", "lastCheck": "2026-05-16T10:00:00Z" },
  { "name": "session_store", "status": "healthy", "lastCheck": "2026-05-16T10:00:00Z" },
  { "name": "model", "status": "degraded", "lastCheck": "2026-05-16T10:00:00Z", "error": "timeout" }
]
```

### Multi-User

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/users/paired` | List paired users |
| POST | `/ui/api/users/{id}/unpair` | Unpair a user |
| POST | `/ui/api/users/{id}/heartbeat` | Update user heartbeat interval |
| GET | `/ui/api/users/groups` | Get group routing assignments |
| POST | `/ui/api/users/groups` | Save a group routing assignment |
| GET | `/ui/api/users/activity` | User activity data |

### Config Encryption

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/encryption/status` | Encryption status (enabled, key path, encrypted field count) |
| GET | `/ui/api/encryption/sensitive-fields` | List sensitive fields in config |
| POST | `/ui/api/encryption/encrypt-all` | Encrypt all sensitive fields |
| POST | `/ui/api/encryption/key-path` | Save encryption key path to config |

**Status response:**
```json
{
  "enabled": true,
  "keyPath": "/path/to/encryption.key",
  "encryptedFields": 5
}
```

### Log Management

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/logs/storage` | Log storage metrics (total size, file count) |
| GET | `/ui/api/logs/files` | List log files with sizes and dates |
| GET | `/ui/api/logs/files/{filename}` | Download a specific log file |
| POST | `/ui/api/logs/clear-old` | Delete log files beyond retention period |

### System

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/system/info` | System info (version, uptime, OS, memory) |
| GET | `/ui/api/system/restart-status` | Current restart state |
| POST | `/ui/api/system/restart` | Trigger graceful restart (SIGUSR1) |

**System info response:**
```json
{
  "version": "1.0.0",
  "uptime_secs": 86400,
  "os": "macos",
  "memory_mb": 128,
  "config_path": "/Users/user/.adk-gateway/gateway.json"
}
```

### ACP (Agent Communication Protocol)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ui/api/acp/agents` | List configured ACP agents |
| POST | `/ui/api/acp/agents` | Add an ACP agent configuration |
| DELETE | `/ui/api/acp/agents/{id}` | Remove an ACP agent |
| GET | `/ui/api/acp/enabled` | Check if ACP feature is enabled |

---

## Error Responses

All API errors return JSON:

```json
{ "error": "description of what went wrong" }
```

| Status | Meaning |
|--------|---------|
| 400 | Invalid request body or parameters |
| 401 | Authentication required or invalid |
| 404 | Resource not found |
| 500 | Internal server error |
