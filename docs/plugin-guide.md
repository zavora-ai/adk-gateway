# Plugin Development Guide

## Overview

Plugins extend gateway behavior through lifecycle hooks. They can inspect, modify, or short-circuit the message processing pipeline.

## Lifecycle Hooks

| Hook | When | Can Short-Circuit? |
|------|------|-------------------|
| `before_run` | Before agent execution starts | Yes |
| `after_run` | After agent execution completes | No |
| `on_user_message` | When a user message arrives | Yes |
| `on_event` | On each event in the stream | No |
| `before_agent` | Before agent turn | Yes |
| `after_agent` | After agent turn | No |
| `before_model` | Before LLM call | Yes |
| `after_model` | After LLM response | No |
| `before_tool` | Before tool execution | Yes |
| `after_tool` | After tool execution | No |

## Configuration

```json5
{
  "plugins": [
    {
      "name": "rate-limiter",
      "enabled": true,
      "config": {
        "maxPerMinute": 10
      }
    },
    {
      "name": "content-filter",
      "enabled": true,
      "config": {
        "blockedPatterns": ["spam"]
      }
    }
  ]
}
```

## Execution Order

Hooks fire in the order plugins are listed in the config. If a plugin returns a short-circuit response, subsequent plugins and processing are skipped.

## Error Handling

- If a plugin fails during initialization, the gateway logs the error and continues without that plugin
- Plugin errors during hook execution are logged but don't crash the gateway
- Short-circuit responses bypass remaining pipeline stages

## Examples

### Logging Plugin

A plugin that logs all user messages:

```rust
// Hooks into on_user_message to log incoming messages
// Does not short-circuit — passes messages through
```

### Rate Limiter

A plugin that limits messages per user:

```rust
// Hooks into on_user_message
// Tracks message counts per user in a sliding window
// Short-circuits with "rate limited" response when exceeded
```

### Content Filter

A plugin that blocks messages matching patterns:

```rust
// Hooks into on_user_message
// Checks message text against blocked patterns
// Short-circuits with rejection message on match
```
