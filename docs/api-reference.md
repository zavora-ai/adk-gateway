# API Reference

## HTTP Endpoints

All endpoints are served on the configured gateway port (default: 18789).

### Health Check

```
GET /health
```

Returns gateway health status.

**Response:** `200 OK`
```json
{ "status": "healthy", "uptime_secs": 3600 }
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

Returns Prometheus-compatible metrics:

```
# HELP adk_gateway_messages_total Total messages processed
# TYPE adk_gateway_messages_total counter
adk_gateway_messages_total{channel="telegram",status="success"} 42

# HELP adk_gateway_active_sessions Current active sessions
# TYPE adk_gateway_active_sessions gauge
adk_gateway_active_sessions 5

# HELP adk_gateway_errors_total Total errors by channel
# TYPE adk_gateway_errors_total counter
adk_gateway_errors_total{channel="telegram"} 2

# HELP adk_gateway_tokens_total Total tokens by model and direction
# TYPE adk_gateway_tokens_total counter
adk_gateway_tokens_total{model="gpt-4",direction="input"} 1000
```

### Webhooks

```
POST /hooks/inbound
Authorization: Bearer <token>
Content-Type: application/json

{
  "text": "Hello, agent!",
  "channel": "telegram",     // optional: deliver response via channel
  "target": "user123",       // optional: target user/group
  "metadata": {}             // optional: arbitrary metadata
}
```

**Response (no delivery target):**
```json
{ "status": "ok", "response": "Agent's response text" }
```

**Response (with delivery target):**
```json
{ "status": "delivered" }
```

**Error (invalid token):** `401 Unauthorized`

### Control Panel

```
GET /ui              # Dashboard
GET /ui/sessions     # Active sessions
GET /ui/config       # Current config (redacted)
GET /ui/logs         # Recent log entries
```

### WebSocket Events

```
GET /ws/events
```

Streams gateway events in real time via WebSocket:

```json
{"type": "message_received", "channel": "telegram", "user": "user123"}
{"type": "response_sent", "channel": "telegram", "latency_ms": 150}
{"type": "error", "channel": "slack", "error": "connection timeout"}
```
