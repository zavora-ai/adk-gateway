# AWP (Agentic Web Protocol) Guide

adk-gateway implements the Agentic Web Protocol, making your gateway discoverable and interactable by AI agents. When enabled, the gateway serves a standard set of AWP endpoints alongside its existing HTTP API.

## Quick Start

### 1. Create `business.toml`

This file describes your site to AI agents. Place it next to your gateway config:

```toml
site_name = "My Business"
site_description = "Online store for handmade ceramics"
domain = "mybusiness.com"

[[capabilities]]
name = "product_search"
description = "Search the product catalog"
endpoint = "/api/products/search"
method = "GET"
access_level = "anonymous"

[[capabilities]]
name = "place_order"
description = "Place an order for a product"
endpoint = "/api/orders"
method = "POST"
access_level = "known"

[[policies]]
name = "privacy"
description = "We do not share visitor data with third parties."
policy_type = "privacy"

[[policies]]
name = "returns"
description = "30-day return policy on unused items."
policy_type = "returns"
```

### 2. Enable AWP in your gateway config

```json5
{
  "agent": { "model": "gemini/gemini-2.5-flash" },
  "awp": {
    "enabled": true,
    "business_toml": "business.toml",
    "hot_reload": true
  }
}
```

### 3. Start the gateway

```bash
adk-gateway --config my-config.json
```

You should see in the logs:

```
AWP business context loaded (site=My Business, capabilities=2)
AWP endpoints registered: /.well-known/awp.json, /awp/manifest, /awp/health, /awp/a2a, /awp/events/*, /awp/consent/*
```

## Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `awp.enabled` | bool | `false` | Enable AWP protocol endpoints |
| `awp.business_toml` | string | `"business.toml"` | Path to business context file (relative to config dir) |
| `awp.hot_reload` | bool | `true` | Watch business.toml for changes and reload automatically |

## Endpoints

### Discovery

**`GET /.well-known/awp.json`**

The standard AWP discovery document. AI agents check this URL first to determine if a site speaks AWP.

```bash
curl http://localhost:18789/.well-known/awp.json
```

```json
{
  "version": { "major": 1, "minor": 0 },
  "siteName": "My Business",
  "siteDescription": "Online store for handmade ceramics",
  "capabilityManifestUrl": "/awp/manifest",
  "a2aEndpointUrl": "/awp/a2a",
  "eventsEndpointUrl": "/awp/events/subscribe",
  "healthEndpointUrl": "/awp/health",
  "supportedTrustLevels": ["anonymous", "known", "partner", "internal"]
}
```

### Capability Manifest

**`GET /awp/manifest`**

JSON-LD capability manifest describing what the site offers.

```bash
curl http://localhost:18789/awp/manifest
```

```json
{
  "@context": "https://schema.org",
  "@type": "WebAPI",
  "name": "My Business",
  "description": "Online store for handmade ceramics",
  "capabilities": [
    {
      "name": "product_search",
      "description": "Search the product catalog",
      "endpoint": "/api/products/search",
      "method": "GET"
    },
    {
      "name": "place_order",
      "description": "Place an order for a product",
      "endpoint": "/api/orders",
      "method": "POST"
    }
  ]
}
```

### Health

**`GET /awp/health`**

Reports the AWP runtime health state. The gateway automatically transitions this when LLM calls succeed or fail.

```bash
curl http://localhost:18789/awp/health
```

```json
{
  "state": "healthy",
  "message": "service started",
  "timestamp": "2026-04-24T10:00:00Z"
}
```

States: `healthy` → `degrading` → `degraded` → `healthy`

### A2A (Agent-to-Agent)

**`POST /awp/a2a`**

Accepts typed A2A messages from external agents.

```bash
curl -X POST http://localhost:18789/awp/a2a \
  -H "Content-Type: application/json" \
  -d '{"id": "msg-001", "type": "awp:InvokeCapability", "payload": {"capability": "product_search", "query": "blue mug"}}'
```

```json
{
  "status": "acknowledged",
  "messageId": "msg-001"
}
```

### Event Subscriptions

**`POST /awp/events/subscribe`** — Create a subscription

```bash
curl -X POST http://localhost:18789/awp/events/subscribe \
  -H "Content-Type: application/json" \
  -d '{
    "subscriber": "agent-007",
    "callbackUrl": "https://my-agent.example.com/awp-events",
    "eventTypes": ["health.changed"],
    "secret": "my-hmac-secret"
  }'
```

**`GET /awp/events/subscriptions`** — List all subscriptions

```bash
curl http://localhost:18789/awp/events/subscriptions
```

**`DELETE /awp/events/subscriptions/{id}`** — Remove a subscription

```bash
curl -X DELETE http://localhost:18789/awp/events/subscriptions/01234567-89ab-cdef-0123-456789abcdef
```

### Consent

AWP requires explicit consent before cross-channel session linking or proactive outreach.

**`POST /awp/consent`** — Capture consent

```bash
curl -X POST http://localhost:18789/awp/consent \
  -H "Content-Type: application/json" \
  -d '{"subject": "user-123", "purpose": "session_continuity"}'
```

**`GET /awp/consent/check`** — Check consent status

```bash
curl "http://localhost:18789/awp/consent/check?subject=user-123&purpose=session_continuity"
```

```json
{
  "subject": "user-123",
  "purpose": "session_continuity",
  "consented": true
}
```

**`POST /awp/consent/revoke`** — Revoke consent

```bash
curl -X POST http://localhost:18789/awp/consent/revoke \
  -H "Content-Type: application/json" \
  -d '{"subject": "user-123", "purpose": "session_continuity"}'
```

## Version Negotiation

All AWP endpoints support version negotiation via the `AWP-Version` header. The gateway responds with the version it supports:

```bash
curl -H "AWP-Version: 1.0" http://localhost:18789/.well-known/awp.json
# Response header: AWP-Version: 1.0
```

If a client requests an incompatible major version, the gateway returns `406 Not Acceptable`.

## Trust Levels

AWP defines four trust levels. The gateway assigns trust based on the `Authorization` header:

| Trust Level | How to get it | Rate Limit |
|-------------|---------------|------------|
| `anonymous` | No auth header | 30 req/min |
| `known` | `Authorization: Bearer <token>` or `Authorization: ApiKey <key>` | 120 req/min |
| `partner` | Verified external agent (future: DID verification) | 600 req/min |
| `internal` | Mesh agents only | Unlimited |

## Health Integration

The gateway automatically reports LLM health to the AWP health state machine:

- When an LLM call succeeds → transitions to `healthy`
- When an LLM call fails → transitions to `degrading`

This means `GET /awp/health` reflects the real-time availability of the AI backend, not just whether the HTTP server is up.

## Hot Reload

When `hot_reload` is enabled (default), the gateway watches `business.toml` for changes and reloads it automatically. You can update capabilities, policies, or site metadata without restarting the gateway.

## Testing

### Unit tests

```bash
cargo test --lib -- awp
```

Runs 8 tests covering config, state building, route merging, health reporting, and a real HTTP request through the merged router.

### Manual testing with curl

Start the gateway with AWP enabled, then:

```bash
# 1. Discovery
curl -s http://localhost:18789/.well-known/awp.json | jq .

# 2. Manifest
curl -s http://localhost:18789/awp/manifest | jq .

# 3. Health
curl -s http://localhost:18789/awp/health | jq .

# 4. A2A message
curl -s -X POST http://localhost:18789/awp/a2a \
  -H "Content-Type: application/json" \
  -d '{"id":"test-1"}' | jq .

# 5. Consent lifecycle
curl -s -X POST http://localhost:18789/awp/consent \
  -H "Content-Type: application/json" \
  -d '{"subject":"user1","purpose":"analytics"}' | jq .

curl -s "http://localhost:18789/awp/consent/check?subject=user1&purpose=analytics" | jq .

curl -s -X POST http://localhost:18789/awp/consent/revoke \
  -H "Content-Type: application/json" \
  -d '{"subject":"user1","purpose":"analytics"}' | jq .

# 6. Event subscription
curl -s -X POST http://localhost:18789/awp/events/subscribe \
  -H "Content-Type: application/json" \
  -d '{"subscriber":"test","callbackUrl":"https://example.com/hook","eventTypes":["health.changed"],"secret":"s3cret"}' | jq .

curl -s http://localhost:18789/awp/events/subscriptions | jq .

# 7. Version negotiation
curl -s -H "AWP-Version: 2.0" http://localhost:18789/.well-known/awp.json
# Returns 406 Not Acceptable
```

### AWP conformance test (future)

```bash
awp test conformance --site http://localhost:18789
```
