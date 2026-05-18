# Changelog

All notable changes to this project will be documented in this file.

## [0.8.2] — 2026-05-17

### Added

- **Coding Agents UI** — Full management interface for coding agents in the Control Panel at `/ui/coding-agents`. Includes agent list with real-time status, onboarding wizard with in-browser CLI installation, task history, cost statistics, task delegation, and agent configuration.
- **Onboarding Wizard** — 5-step guided registration: backend selection, CLI verification with install button, authentication, workspace selection, and finalization.
- **In-Browser Install** — "Install" button runs CLI install commands server-side and displays output in real-time. Supports platform-specific commands (macOS/Linux/Windows).
- **Activate & Test** — Post-install button triggers CLI verification to confirm installation succeeded.
- **Backend Definitions** — Extensible backend catalog: Claude Code, Kiro CLI, Codex CLI, OpenCode, GitHub Copilot CLI. Platform-specific install instructions per backend.
- **Cross-Platform PATH Resolution** — CLI verification searches `~/go/bin`, `~/.local/bin`, `~/.cargo/bin`, `/opt/homebrew/bin` in addition to system PATH.

### Changed

- Upgraded to adk-rust 0.8.2 — MCP toolkit no longer sanitizes schemas at the toolset layer. Raw schemas are returned verbatim; normalization is handled per-provider by SchemaAdapter implementations at request time.
- Backend definition config supports optional `installInstructionsWindows` and `installInstructionsLinux` fields for platform-specific install commands.

### Fixed

- Agent list view normalizes backend status values (`unknown`, `running`, `active`) to valid connection status (`connected`, `disconnected`, `error`).
- Agent detail API response normalized to match frontend type expectations (camelCase → snake_case field mapping).
- Tasks endpoint response normalized from flat array to paginated format expected by TaskHistoryTable.
- Wizard registration payload transformed to match backend's expected camelCase format with required `id` field.
- CLI verification endpoint corrected from non-existent `/verify-cli/:type` to actual `/onboarding/check-install`.
- Backends list endpoint response unwrapped from `{ ok, backends }` envelope to provide `data` field for useApi hook.

## [0.8.1] — 2026-05-16

### Added

- **Tool Approval** — Dangerous tool calls now pause and present ✅/❌ inline buttons in Telegram for explicit user approval. Configurable per-tool and per-category with 120s auto-reject timeout.
- **Stale Context Detection** — Welcome-back messages after idle periods (default 4h) with pending task summaries and heartbeat alerts.
- **Rate Limiting** — Sliding-window rate limiter (10 calls/5s default) prevents runaway tool loops. Auto-terminates after 3 triggers per request.
- **Health Monitoring** — Periodic health checks (every 60s) with alerting via webhook or Telegram on 3 consecutive failures. Recovery notifications included.
- **Config Encryption** — AES-256-GCM encryption for sensitive config values at rest. CLI command `config encrypt` encrypts secrets in-place. Encrypted values use `enc:` prefix.
- **Zero-Downtime Restart** — SIGUSR1 triggers graceful restart with drain phases (stop accepting → drain in-flight → exit). Configurable drain timeout (default 30s).
- **Multi-User** — Multiple paired users per channel with independent sessions, per-user heartbeat delivery, and thread-scoped group chat responses.
- **Heartbeat V2** — Session-integrated heartbeat replaces the isolated cron-job approach. Full conversation context, per-user scheduling, and automatic history cleanup for non-alert responses.
- **ACP Integration** — Delegate coding tasks to Claude Code or Codex via Agent Communication Protocol. Per-agent ACP endpoint configuration with 300s timeout and progress messages.
- **Docker Deployment** — Multi-stage Dockerfile producing <100MB images. Supports feature flag build args and includes HEALTHCHECK.
- **Systemd Service** — Production-ready `adk-gateway.service` with `Type=notify`, readiness signaling (`sd_notify`), auto-restart, and resource limits.
- **Launchd Plist** — macOS `com.zavora.adk-gateway.plist` for launch agent deployment.
- **Provider-Aware Schema Sanitization** — JSON Schema transformations now applied only for Gemini. OpenAI and Anthropic receive unmodified schemas.
- **Configurable Max Iterations** — Runner `max_iterations` now configurable via gateway config (1–1000, default 100). Per-request override supported.
- **Structured Log Rotation** — Daily rotation with configurable retention (default 7 days) and size-based rotation (default 100MB). Supports JSON and pretty formats.

### Changed

- Heartbeat system completely rewritten to run within user sessions instead of isolated contexts.
- Schema sanitization moved from MCP toolset layer to provider-specific model adapters.
- Runner iteration limit is now a gateway config parameter rather than a hardcoded constant.

### Fixed

- Tool schemas with `exclusiveMinimum`/`exclusiveMaximum` no longer cause Gemini API errors.
- Array-typed `type` fields in JSON Schema (e.g., `["string", "null"]`) correctly converted to `nullable: true` for Gemini.

## [0.7.0] — 2026-04-28

Initial public release with 5 channels, 14 LLM providers, multi-agent, persistent memory, RAG, graph workflows, scheduled tasks, MCP integration, AWP protocol, and React control panel.
