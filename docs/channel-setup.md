# Channel Setup Guide

## Telegram

### 1. Create a Bot

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot` and follow the prompts
3. Copy the bot token

### 2. Configure

```json5
{
  "channels": {
    "telegram": {
      "botToken": "${TELEGRAM_BOT_TOKEN}",
      "dmPolicy": "open",        // or "pairing", "allowlist", "disabled"
      "allowFrom": ["@username"], // for allowlist mode
      "streamMode": "partial",   // "partial" for streaming edits, "complete" for batch
      "groups": {
        "my-group": { "requireMention": true }
      }
    }
  }
}
```

### 3. DM Policies

- **open** — Anyone can message the bot
- **allowlist** — Only users in `allowFrom` list
- **pairing** — Users must enter a pairing code first
- **disabled** — DMs are blocked

### Multi-Account

```json5
{
  "channels": {
    "telegram": { "botToken": "${BOT1_TOKEN}", "accountId": "bot1" },
    "telegramAccounts": [
      { "botToken": "${BOT2_TOKEN}", "accountId": "bot2" }
    ]
  }
}
```

---

## Slack

### 1. Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps)
2. Create a new app with Socket Mode enabled
3. Add bot scopes: `chat:write`, `app_mentions:read`, `im:history`, `im:read`, `im:write`
4. Generate an App-Level Token with `connections:write` scope
5. Install the app to your workspace

### 2. Configure

```json5
{
  "channels": {
    "slack": {
      "botToken": "${SLACK_BOT_TOKEN}",
      "appToken": "${SLACK_APP_TOKEN}",
      "dmPolicy": "open"
    }
  }
}
```

### 3. Socket Mode

Slack uses Socket Mode (WebSocket) — no public URL needed. The gateway connects outbound to Slack's servers.

### Multi-Account

```json5
{
  "channels": {
    "slack": { "botToken": "${SLACK1_BOT}", "appToken": "${SLACK1_APP}", "accountId": "workspace1" },
    "slackAccounts": [
      { "botToken": "${SLACK2_BOT}", "appToken": "${SLACK2_APP}", "accountId": "workspace2" }
    ]
  }
}
```
