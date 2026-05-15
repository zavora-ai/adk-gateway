# Implementation Tasks — Phase 2 Complete

## Task 1: Provider-Aware Schema Sanitization
- **Priority**: P0
- **Estimated effort**: M
- **Dependencies**: None
- **Requirements**: Req 5.1, Req 5.2, Req 5.3, Req 5.4, Req 5.5, Req 5.6, Req 5.7, Req 5.8
- **Status**: pending

### Description
Move Gemini-specific JSON Schema fixes out of the MCP tool layer and into the model adapter. Create `src/schema_sanitizer.rs` with a `SchemaSanitizer` trait, a `GeminiSanitizer` implementation (removes/transforms `exclusiveMinimum`, `exclusiveMaximum`, array-typed `items`, `propertyNames`, array-typed `type`), and an `IdentitySanitizer` for providers that accept standard JSON Schema (OpenAI, Anthropic). Remove the existing `sanitize_schema` function from the `adk-tool` layer. Store original schemas unmodified and apply provider-specific transformations only at model invocation time.

### Acceptance Criteria
- [ ] `SchemaSanitizer` trait defined with `fn sanitize(&self, schema: &serde_json::Value) -> serde_json::Value`
- [ ] `GeminiSanitizer` removes `propertyNames` from schemas
- [ ] `GeminiSanitizer` converts `exclusiveMinimum: N` → `minimum: N+1` for integer types
- [ ] `GeminiSanitizer` converts `exclusiveMaximum: N` → `maximum: N-1` for integer types
- [ ] `GeminiSanitizer` converts array-typed `type` (e.g., `["string", "null"]`) to single type with `nullable: true`
- [ ] `GeminiSanitizer` converts array-typed `items` to single schema (first element)
- [ ] `IdentitySanitizer` returns schema unchanged (clone)
- [ ] Existing `sanitize_schema` removed from MCP/adk-tool layer
- [ ] Original MCP tool schemas stored unmodified in gateway state
- [ ] Property-based tests validate Properties 4, 5, and 6 from design

### Files to modify/create
- `src/schema_sanitizer.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/mcp_manager.rs` (remove sanitize_schema, store raw schemas)
- `src/runner_bridge.rs` (apply sanitizer at model invocation time)
- `tests/schema_sanitizer_test.rs` (create)

---

## Task 2: Runner Max Iterations
- **Priority**: P0
- **Estimated effort**: M
- **Dependencies**: None
- **Requirements**: Req 6.1, Req 6.2, Req 6.3, Req 6.4, Req 6.5, Req 6.6
- **Status**: pending

### Description
Add `max_iterations` configuration to the Runner. The gateway passes a default of 25 iterations per request. Validate that the value is within [1, 1000] at config load time. The Runner terminates the tool-call loop when the iteration count reaches `max_iterations` and returns a partial result with a `max_iterations_reached` indicator. Include iteration count in response metadata.

### Acceptance Criteria
- [ ] `GatewayRunnerConfig` struct with `max_iterations: u32` field (default: 25)
- [ ] Validation rejects values < 1 or > 1000 with a clear error
- [ ] Gateway passes `max_iterations` to the Runner on every request
- [ ] Runner terminates loop at exactly `max_iterations` and returns partial result
- [ ] Response metadata includes `iteration_count` and `max_iterations_reached: bool`
- [ ] Per-request override of `max_iterations` supported via gateway config
- [ ] Property-based tests validate Property 7 from design
- [ ] Unit tests cover boundary values (0, 1, 1000, 1001)

### Files to modify/create
- `src/config.rs` (add `GatewayRunnerConfig`, validation)
- `src/runner_bridge.rs` (pass max_iterations to Runner)
- `reference/adk-rust/` (upstream Runner changes if needed)
- `tests/properties.rs` (add Property 7 tests)

---

## Task 3: Rate Limiter
- **Priority**: P0
- **Estimated effort**: S
- **Dependencies**: None
- **Requirements**: Req 3.1, Req 3.2, Req 3.3, Req 3.4, Req 3.5
- **Status**: pending

### Description
Create `src/rate_limiter.rs` with a sliding-window rate limiter for tool invocations. Default: 10 calls per 5-second window. When exceeded, pause for a configurable cooldown (default: 3s). If triggered 3 times in a single request, terminate the request and notify the user. Integrate into the `after_tool_callback` in the runner bridge. Replace the current dedup check with this more robust mechanism. Log each trigger event with tool names and invocation count.

### Acceptance Criteria
- [ ] `RateLimiter` struct with sliding window using `VecDeque<Instant>`
- [ ] `RateLimitDecision` enum: `Allow`, `Pause { duration }`, `Terminate { reason }`
- [ ] `record_invocation` correctly counts calls within the sliding window
- [ ] Pause triggered when calls exceed threshold within window
- [ ] Termination triggered after 3 pauses in a single request
- [ ] Each trigger event logged with tool names and count
- [ ] Per-agent rate limit configuration supported
- [ ] Integrated into `after_tool_callback` replacing current dedup logic
- [ ] Property-based tests validate Property 3 from design

### Files to modify/create
- `src/rate_limiter.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (add `RateLimitConfig`)
- `src/runner_bridge.rs` (integrate into after_tool_callback)
- `tests/properties.rs` (add Property 3 tests)

---

## Task 4: Tool Approval Flow
- **Priority**: P1
- **Estimated effort**: L
- **Dependencies**: None
- **Requirements**: Req 1.1, Req 1.2, Req 1.3, Req 1.4, Req 1.5, Req 1.6, Req 1.7
- **Status**: pending

### Description
Create `src/tool_approval.rs` implementing an interactive tool approval flow. When the Runner invokes a tool marked as `requires_approval`, execution pauses and a Telegram message with inline keyboard buttons (✅ Approve / ❌ Reject) is sent to the user. The system waits up to 120 seconds for a response. Implement the `ToolApprovalService` trait, callback handler for Telegram inline button presses, and default classification of dangerous tools (file writes, shell exec, destructive ops). Support custom approval rules from config.

### Acceptance Criteria
- [ ] `ToolApprovalService` trait with `requires_approval`, `request_approval`, `handle_callback`
- [ ] `ApprovalState` enum: `Pending`, `Approved`, `Rejected`, `TimedOut`
- [ ] Telegram inline keyboard with ✅ Approve / ❌ Reject buttons sent on dangerous tool calls
- [ ] Callback handler processes button presses and resolves pending approvals
- [ ] 120-second timeout auto-rejects and notifies user
- [ ] Default dangerous categories: `fs_write`, `fs_delete`, `shell_exec`, `run_command`
- [ ] Custom approval rules from config override defaults
- [ ] "⏳ Waiting for approval..." status message displayed while pending
- [ ] Integration with `before_tool_callback` in runner bridge
- [ ] Property-based tests validate Property 1 from design

### Files to modify/create
- `src/tool_approval.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (add `ApprovalConfig`)
- `src/runner_bridge.rs` (integrate into before_tool_callback)
- `src/channels/telegram.rs` (inline keyboard support, callback handler)
- `tests/tool_approval_test.rs` (create)

---

## Task 5: Stale Context Detection
- **Priority**: P1
- **Estimated effort**: S
- **Dependencies**: None
- **Requirements**: Req 2.1, Req 2.2, Req 2.3, Req 2.4, Req 2.5
- **Status**: pending

### Description
Create `src/stale_context.rs` implementing idle period detection. On each incoming message, check the user's `last_activity` timestamp from session history. If the idle period exceeds the configured threshold (default: 4 hours), build and send a welcome-back message summarizing pending tasks, heartbeat alerts, and time since last interaction. If no pending items exist, send a brief acknowledgment.

### Acceptance Criteria
- [ ] `StaleContextDetector` struct with configurable `idle_threshold_secs` (default: 14400)
- [ ] `is_stale` function correctly compares timestamps against threshold
- [ ] `build_welcome_back` includes: idle duration, pending task count, heartbeat alerts
- [ ] Brief acknowledgment sent when no pending items or alerts exist
- [ ] Custom idle threshold supported via config
- [ ] Integrated into message processing pipeline (checked on message arrival)
- [ ] Property-based tests validate Property 2 from design

### Files to modify/create
- `src/stale_context.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (add `StaleContextConfig`)
- `src/message_handler.rs` (integrate stale check on message arrival)
- `tests/properties.rs` (add Property 2 tests)

---

## Task 6: Heartbeat V2
- **Priority**: P1
- **Estimated effort**: XL
- **Dependencies**: Task 2, Task 7
- **Requirements**: Req 7.1, Req 7.2, Req 7.3, Req 7.4, Req 7.5, Req 7.6, Req 7.7, Req 7.8
- **Status**: pending

### Description
Create `src/heartbeat_v2.rs` replacing the current cron-based heartbeat. The new heartbeat runs within the user's active session with full conversation context. It injects a heartbeat prompt into the session, processes it through the Runner, and classifies the response. "HEARTBEAT_OK" responses are stripped from history; actionable alerts are retained and delivered to the user. Implement per-user scheduling with independent intervals and cancellation tokens. If no active session exists, create a temporary one for the check.

### Acceptance Criteria
- [ ] `HeartbeatV2` struct with per-user `DashMap<String, HeartbeatSchedule>`
- [ ] Heartbeat executes within user's session with full history context
- [ ] Heartbeat prompt injected into session and processed through Runner
- [ ] `classify_response` correctly identifies "HEARTBEAT_OK" vs actionable alerts
- [ ] `strip_heartbeat_turns` removes OK turns, retains alert turns, never touches regular turns
- [ ] Per-user scheduling with independent intervals
- [ ] Cancellation support via `CancellationToken`
- [ ] Temporary session created when no active session exists
- [ ] Current cron-based heartbeat replaced entirely
- [ ] Property-based tests validate Property 8 from design

### Files to modify/create
- `src/heartbeat_v2.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/heartbeat.rs` (deprecate/remove old implementation)
- `src/config.rs` (add heartbeat V2 config)
- `src/runner_bridge.rs` (support heartbeat prompt injection)
- `tests/heartbeat_v2_test.rs` (create)
- `tests/properties.rs` (add Property 8 tests)

---

## Task 7: Multi-User Support
- **Priority**: P1
- **Estimated effort**: L
- **Dependencies**: Task 6
- **Requirements**: Req 8.1, Req 8.2, Req 8.3, Req 8.4, Req 8.5, Req 8.6, Req 8.7
- **Status**: pending

### Description
Create `src/multi_user.rs` extending the pairing system to support multiple users per channel. Each paired user gets an independent session, heartbeat schedule, and delivery target. Implement per-user session isolation, thread-aware group responses, and agent routing for group chats. Removing a user stops their heartbeat and session without affecting others.

### Acceptance Criteria
- [ ] `MultiUserManager` with `DashMap<(ChannelType, String), PairedUser>`
- [ ] `add_user` registers new user without affecting existing pairings
- [ ] `remove_user` stops heartbeat and session for that user only
- [ ] Per-user session isolation (separate `Session_History` per user)
- [ ] Thread context used to scope responses in group chats
- [ ] `Agent_Router` routes messages from specific groups to designated agents
- [ ] Heartbeat V2 delivers alerts independently per paired user
- [ ] Property-based tests validate Properties 9 and 10 from design

### Files to modify/create
- `src/multi_user.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (add multi-user and routing config)
- `src/pairing.rs` (extend to support multiple users)
- `src/channels/telegram.rs` (thread-aware responses)
- `src/heartbeat_v2.rs` (per-user delivery integration)
- `tests/properties.rs` (add Properties 9, 10 tests)

---

## Task 8: ACP Integration
- **Priority**: P2
- **Estimated effort**: L
- **Dependencies**: Task 1
- **Requirements**: Req 4.1, Req 4.2, Req 4.3, Req 4.4, Req 4.5, Req 4.6, Req 4.7
- **Status**: pending

### Description
Add `adk-acp` as an optional dependency behind the `acp` feature flag. Create `src/acp.rs` (feature-gated) implementing ACP tool registration and execution. Register ACP tools per-agent based on config. Support task delegation to Claude Code or Codex with configurable timeout (default: 300s). Send periodic progress messages to the user during long-running tasks. Handle endpoint errors gracefully without crashing.

### Acceptance Criteria
- [ ] `adk-acp` added as optional dependency with `acp` feature flag
- [ ] `AcpTool` struct with `execute` method for task delegation
- [ ] ACP tools registered per-agent based on gateway config
- [ ] Support for Claude Code and Codex agent types
- [ ] Configurable timeout (default: 300s) for long-running tasks
- [ ] Periodic progress messages sent to user during execution
- [ ] Graceful error handling: unreachable endpoint returns descriptive error, no crash
- [ ] Feature-gated compilation (`#[cfg(feature = "acp")]`)

### Files to modify/create
- `src/acp.rs` (create, feature-gated)
- `src/lib.rs` (add conditional module declaration)
- `Cargo.toml` (add adk-acp optional dependency, acp feature)
- `src/config.rs` (add `AcpConfig` per-agent)
- `src/runner_bridge.rs` (register ACP tools in agent toolset)

---

## Task 9: Health Monitor
- **Priority**: P2
- **Estimated effort**: M
- **Dependencies**: None
- **Requirements**: Req 11.1, Req 11.2, Req 11.3, Req 11.4, Req 11.5
- **Status**: pending

### Description
Create `src/health_monitor.rs` implementing periodic health checks for gateway components (channel connectivity, model reachability, session store availability). Alert after 3 consecutive failures via configured channel (webhook POST or Telegram message to admin). Emit recovery notifications when a failing component recovers. Expose per-component health status via the existing `/health` endpoint.

### Acceptance Criteria
- [ ] `HealthMonitor` with periodic checks (default: every 60 seconds)
- [ ] Checks cover: channel connectivity, model reachability, session store
- [ ] Alert emitted after 3 consecutive failures for a component
- [ ] Recovery notification emitted when component transitions from failed to healthy
- [ ] No duplicate alerts or recoveries for the same state
- [ ] Webhook alerting: POST JSON with component name, status, failure count, timestamp
- [ ] Telegram alerting: message to configured admin user
- [ ] `/health` endpoint returns per-component breakdown
- [ ] Property-based tests validate Property 11 from design

### Files to modify/create
- `src/health_monitor.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (add `HealthMonitorConfig`)
- `src/api.rs` (extend `/health` endpoint)
- `tests/health_monitor_test.rs` (create)
- `tests/properties.rs` (add Property 11 tests)

---

## Task 10: Log Rotation Enhancement
- **Priority**: P2
- **Estimated effort**: S
- **Dependencies**: None
- **Requirements**: Req 12.1, Req 12.2, Req 12.3, Req 12.4, Req 12.5
- **Status**: pending

### Description
Extend `src/telemetry.rs` with retention-based log cleanup and size-based rotation. Add configurable retention period (default: 7 days) that deletes older log files. Support size-based rotation (default: 100MB per file). Add configurable log format via `LOG_FORMAT` environment variable (`json` or `pretty`).

### Acceptance Criteria
- [ ] Daily log rotation with configurable retention period (default: 7 days)
- [ ] Files older than retention period automatically deleted
- [ ] Size-based rotation at configurable threshold (default: 100MB)
- [ ] `LOG_FORMAT=pretty` outputs human-readable logs
- [ ] `LOG_FORMAT=json` (default) outputs structured JSON logs
- [ ] `files_to_delete` function correctly identifies expired files
- [ ] Property-based tests validate Property 12 from design

### Files to modify/create
- `src/telemetry.rs` (extend with rotation/retention logic)
- `src/config.rs` (add `LogRotationConfig`)
- `tests/properties.rs` (add Property 12 tests)

---

## Task 11: Config Encryption
- **Priority**: P2
- **Estimated effort**: M
- **Dependencies**: None
- **Requirements**: Req 14.1, Req 14.2, Req 14.3, Req 14.4, Req 14.5, Req 14.6
- **Status**: pending

### Description
Create `src/config_encryption.rs` implementing AES-256-GCM encryption for sensitive config values. Encrypted values use the `enc:` prefix for identification. Provide a CLI command (`adk-gateway config encrypt`) that encrypts plaintext secrets in-place. Detect sensitive fields by convention (names containing `key`, `token`, `secret`, `password`). Fail fast with a clear error if encrypted values are present but no decryption key is available.

### Acceptance Criteria
- [ ] `ConfigEncryption` struct with AES-256-GCM encrypt/decrypt
- [ ] `encrypt` produces `enc:<base64(nonce+ciphertext)>` format
- [ ] `decrypt` correctly reverses encryption (round-trip property)
- [ ] `is_sensitive_field` detects fields containing `key`, `token`, `secret`, `password`
- [ ] `is_encrypted` checks for `enc:` prefix
- [ ] CLI command `adk-gateway config encrypt` encrypts sensitive fields in-place
- [ ] Gateway decrypts encrypted values at startup when key is available
- [ ] Clear error and exit if encrypted values present without decryption key
- [ ] Plaintext and encrypted values coexist during migration
- [ ] Property-based tests validate Properties 14 and 15 from design

### Files to modify/create
- `src/config_encryption.rs` (create)
- `src/lib.rs` (add module declaration)
- `src/config.rs` (integrate decryption at load time)
- `src/cli.rs` (add `config encrypt` subcommand)
- `Cargo.toml` (add `aes-gcm` dependency)
- `tests/config_encryption_test.rs` (create)
- `tests/properties.rs` (add Properties 14, 15 tests)

---

## Task 12: Docker + Systemd Deployment
- **Priority**: P3
- **Estimated effort**: M
- **Dependencies**: Task 9
- **Requirements**: Req 9.1, Req 9.2, Req 9.3, Req 9.4, Req 9.5, Req 10.1, Req 10.2, Req 10.3, Req 10.4, Req 10.5
- **Status**: pending

### Description
Create a production-ready Dockerfile with multi-stage build (Rust builder + minimal runtime). Final image must be under 100MB. Support feature flag build arguments. Add HEALTHCHECK instruction. Create `adk-gateway.service` systemd unit file with `Type=notify`, automatic restart, and resource limits. Integrate `sd_notify(READY=1)` signaling into the gateway startup sequence.

### Acceptance Criteria
- [ ] Dockerfile with multi-stage build: Rust builder + distroless/alpine runtime
- [ ] Final image size < 100MB (excluding mounted volumes)
- [ ] Build arguments for feature flags (`--features browser,postgres`)
- [ ] Configuration from environment variables and/or mounted config file
- [ ] HEALTHCHECK instruction calling `/health` endpoint
- [ ] `adk-gateway.service` with `Type=notify` and `sd_notify(READY=1)` integration
- [ ] Automatic restart on failure with 5-second delay
- [ ] Resource limits (memory, file descriptors) configured in service file
- [ ] Graceful shutdown on SIGTERM with configurable drain timeout (default: 30s)

### Files to modify/create
- `Dockerfile` (create)
- `adk-gateway.service` (create)
- `.dockerignore` (create)
- `src/main.rs` (add sd_notify integration)
- `Cargo.toml` (add sd-notify optional dependency)

---

## Task 13: Zero-Downtime Restart
- **Priority**: P3
- **Estimated effort**: L
- **Dependencies**: Task 12
- **Requirements**: Req 13.1, Req 13.2, Req 13.3, Req 13.4, Req 13.5
- **Status**: pending

### Description
Extend `src/shutdown.rs` with SIGUSR1 handler for graceful restarts. On SIGUSR1, stop accepting new connections while continuing to process in-flight requests. Support socket-based handoff so the new process can bind before the old releases. Implement drain phases with structured logging at each phase (drain-start, drain-complete, shutdown). Force-terminate remaining requests after drain timeout with logged warnings.

### Acceptance Criteria
- [ ] SIGUSR1 signal handler initiates graceful restart
- [ ] New connections rejected after restart signal received
- [ ] In-flight requests continue processing until completion or timeout
- [ ] Socket-based handoff: new process binds before old releases
- [ ] Drain timeout (default: 30s) force-terminates remaining requests
- [ ] Structured log events at each phase: `drain-start`, `drain-complete`, `shutdown`
- [ ] Force-terminated requests logged with request details
- [ ] In-flight count monotonically non-increasing after shutdown initiation
- [ ] Property-based tests validate Property 13 from design

### Files to modify/create
- `src/shutdown.rs` (extend with SIGUSR1 handler, socket handoff)
- `src/main.rs` (register SIGUSR1 signal handler)
- `src/api.rs` (connection acceptance gate)
- `tests/properties.rs` (add Property 13 tests)

---

# UI Implementation Tasks

## Task 14: Tool Approval UI (Telegram Inline Buttons + Control Panel)
- **Priority**: P1
- **Estimated effort**: M
- **Dependencies**: Task 4
- **Requirements**: Req 1.1, Req 1.5, Req 1.6, Req 1.7
- **Status**: pending

### Description
Build the UI surfaces for tool approval: (1) Telegram inline keyboard buttons for approve/reject, (2) a Control Panel page showing pending approvals with approve/reject actions, and (3) a settings section for configuring which tools require approval.

### Acceptance Criteria
- [ ] Telegram message with inline keyboard: `✅ Approve` and `❌ Reject` buttons
- [ ] Message shows tool name, arguments summary, and "⏳ Waiting for approval..." status
- [ ] Button press updates the message to show "✅ Approved" or "❌ Rejected" with timestamp
- [ ] Timeout updates message to "⏰ Timed out (auto-rejected)"
- [ ] Control Panel: new "Approvals" section on Dashboard showing pending approvals count
- [ ] Control Panel: Settings page has "Tool Approval" section with:
  - Toggle to enable/disable approval flow
  - Multi-select list of tools that require approval (pre-populated with defaults)
  - Timeout slider (30s–300s, default 120s)
- [ ] Approval history visible in the Logs page with filter for "approval" events

### Files to modify/create
- `ui/src/pages/Dashboard.tsx` (add pending approvals widget)
- `ui/src/pages/Settings.tsx` (add Tool Approval config section)
- `ui/src/components/ApprovalBadge.tsx` (create — shows pending count)
- `src/channel/telegram.rs` (inline keyboard markup for approve/reject)

---

## Task 15: Stale Context / Welcome Back UI
- **Priority**: P1
- **Estimated effort**: S
- **Dependencies**: Task 5
- **Requirements**: Req 2.1, Req 2.3
- **Status**: pending

### Description
Add a Settings UI section for configuring the stale context detector (idle threshold), and show the "welcome back" state in the Dashboard when applicable.

### Acceptance Criteria
- [ ] Settings page: "Session & Context" section with idle threshold input (hours, default 4)
- [ ] Dashboard: shows "Last active: X hours ago" per paired user when idle > 1h
- [ ] Telegram: welcome-back message formatted with markdown (bold headers, bullet lists)
- [ ] Welcome-back message includes clickable summary: "📋 2 pending tasks, 1 alert"

### Files to modify/create
- `ui/src/pages/Settings.tsx` (add idle threshold config)
- `ui/src/pages/Dashboard.tsx` (add last-active indicators per user)

---

## Task 16: Rate Limiter UI
- **Priority**: P1
- **Estimated effort**: S
- **Dependencies**: Task 3
- **Requirements**: Req 3.1, Req 3.5
- **Status**: pending

### Description
Add rate limiter configuration to the Settings page and show rate-limit events in the Logs page.

### Acceptance Criteria
- [ ] Settings page: "Rate Limiting" section with:
  - Max calls per window (number input, default 10)
  - Window duration (seconds, default 5)
  - Cooldown duration (seconds, default 3)
  - Max triggers before termination (number, default 3)
- [ ] Logs page: rate-limit events highlighted with orange badge "RATE_LIMITED"
- [ ] Dashboard: metric card showing "Rate limits triggered today: N"

### Files to modify/create
- `ui/src/pages/Settings.tsx` (add Rate Limiting section)
- `ui/src/pages/Logs.tsx` (highlight rate-limit events)
- `ui/src/pages/Dashboard.tsx` (add rate-limit metric)

---

## Task 17: ACP Integration UI
- **Priority**: P2
- **Estimated effort**: M
- **Dependencies**: Task 8
- **Requirements**: Req 4.1, Req 4.4
- **Status**: pending

### Description
Add an "ACP Agents" section to the Integrations page for configuring external coding agents (Claude Code, Codex). Show connection status and allow adding/removing ACP endpoints.

### Acceptance Criteria
- [ ] Integrations page: new "ACP Agents" section (only visible when `acp` feature enabled)
- [ ] Table showing configured ACP agents: name, command, status, timeout
- [ ] "Add ACP Agent" form with fields: name, command, working directory, auto-approve toggle, timeout
- [ ] Status badges: Connected / Disconnected / Error
- [ ] Remove button with confirmation dialog
- [ ] Per-agent assignment UI: dropdown on Agent config page to assign ACP tools

### Files to modify/create
- `ui/src/pages/Integrations.tsx` (add ACP Agents section)
- `ui/src/pages/Agents.tsx` (add ACP tool assignment dropdown)
- `ui/src/types/index.ts` (add AcpAgentInfo type)
- `ui/src/api/client.ts` (add ACP CRUD endpoints)

---

## Task 18: Health Monitor UI
- **Priority**: P2
- **Estimated effort**: M
- **Dependencies**: Task 9
- **Requirements**: Req 11.1, Req 11.4, Req 11.5
- **Status**: pending

### Description
Add a "System Health" page or Dashboard section showing per-component health status with history, alert configuration, and recovery timeline.

### Acceptance Criteria
- [ ] Dashboard: health status row showing all components with green/yellow/red indicators
- [ ] Clicking a component opens detail view with:
  - Current status and last check time
  - Consecutive failure count
  - Last 24h health timeline (sparkline or dot chart)
- [ ] Settings page: "Health Monitoring" section with:
  - Check interval (seconds, default 60)
  - Failure threshold (number, default 3)
  - Alert webhook URL input
  - Alert Telegram admin chat ID input
- [ ] Alert history table showing: timestamp, component, event (alert/recovery), message
- [ ] Real-time WebSocket updates for health status changes

### Files to modify/create
- `ui/src/pages/Dashboard.tsx` (add health status row)
- `ui/src/components/HealthTimeline.tsx` (create — sparkline component)
- `ui/src/pages/Settings.tsx` (add Health Monitoring config)
- `ui/src/api/client.ts` (add health status + history endpoints)
- `ui/src/types/index.ts` (add HealthComponent, HealthEvent types)

---

## Task 19: Multi-User Management UI
- **Priority**: P2
- **Estimated effort**: M
- **Dependencies**: Task 7
- **Requirements**: Req 8.1, Req 8.2, Req 8.7
- **Status**: pending

### Description
Extend the Channels page with a "Paired Users" management section showing all paired users, their status, last activity, and actions (unpair, configure heartbeat).

### Acceptance Criteria
- [ ] Channels page: "Paired Users" section showing table of all paired users
- [ ] Table columns: User ID, Channel, Paired At, Last Active, Heartbeat Status, Actions
- [ ] "Unpair" button with confirmation dialog per user
- [ ] Per-user heartbeat toggle (enable/disable) and interval override
- [ ] Sessions page: filter by user to see individual session history
- [ ] Dashboard: "Paired Users" metric card showing count per channel
- [ ] Group chat config: UI for assigning agents to specific groups/threads

### Files to modify/create
- `ui/src/pages/Channels.tsx` (add Paired Users section)
- `ui/src/components/PairedUsersTable.tsx` (create)
- `ui/src/pages/Sessions.tsx` (add user filter)
- `ui/src/pages/Dashboard.tsx` (add paired users metric)
- `ui/src/api/client.ts` (add paired users CRUD endpoints)
- `ui/src/types/index.ts` (extend PairedUser type)

---

## Task 20: Config Encryption UI
- **Priority**: P2
- **Estimated effort**: S
- **Dependencies**: Task 11
- **Requirements**: Req 14.3, Req 14.6
- **Status**: pending

### Description
Add encryption management to the Settings page: show which fields are encrypted, provide a button to encrypt all sensitive fields, and display encryption status.

### Acceptance Criteria
- [ ] Settings page: "Security" section showing encryption status (enabled/disabled)
- [ ] List of sensitive fields with status: 🔒 Encrypted / ⚠️ Plaintext
- [ ] "Encrypt All" button that calls the backend to encrypt plaintext secrets
- [ ] Encryption key file path configuration input
- [ ] Warning banner when encrypted values exist but no key is configured
- [ ] Config page: encrypted values shown as `enc:****` (masked) with "Decrypt to edit" option

### Files to modify/create
- `ui/src/pages/Settings.tsx` (add Security/Encryption section)
- `ui/src/pages/Config.tsx` (mask encrypted values, decrypt-to-edit flow)
- `ui/src/api/client.ts` (add encrypt endpoint)

---

## Task 21: Log Rotation & Monitoring UI
- **Priority**: P3
- **Estimated effort**: S
- **Dependencies**: Task 10
- **Requirements**: Req 12.1, Req 12.2, Req 12.5
- **Status**: pending

### Description
Add log management configuration to Settings and show disk usage/rotation status on the Dashboard.

### Acceptance Criteria
- [ ] Settings page: "Logging" section with:
  - Rotation policy dropdown (Daily / Hourly / Size-based)
  - Retention period (days, default 7)
  - Max file size (MB, default 100)
  - Format toggle (JSON / Pretty)
- [ ] Dashboard: "Log Storage" metric showing total size, file count, oldest file date
- [ ] Logs page: "Download" button for individual log files
- [ ] Logs page: "Clear Old Logs" button with confirmation (deletes beyond retention)

### Files to modify/create
- `ui/src/pages/Settings.tsx` (add Logging section)
- `ui/src/pages/Dashboard.tsx` (add log storage metric)
- `ui/src/pages/Logs.tsx` (add download + clear buttons)
- `ui/src/api/client.ts` (add log management endpoints)

---

## Task 22: Deployment Status UI
- **Priority**: P3
- **Estimated effort**: S
- **Dependencies**: Task 12, Task 13
- **Requirements**: Req 9.5, Req 13.5
- **Status**: pending

### Description
Add a "System" section to the Dashboard showing deployment info, restart status, and Docker/systemd integration state.

### Acceptance Criteria
- [ ] Dashboard: "System Info" card showing:
  - Version, build date, Rust version
  - Deployment mode (Docker / Systemd / Standalone)
  - Process uptime, PID, memory usage
- [ ] Dashboard: "Restart" button (triggers SIGUSR1 graceful restart)
- [ ] During restart: progress indicator showing drain phase (X in-flight requests remaining)
- [ ] Settings page: "Deployment" section showing:
  - Drain timeout configuration
  - Current restart phase (if active)
  - Last restart timestamp and reason

### Files to modify/create
- `ui/src/pages/Dashboard.tsx` (add System Info card + Restart button)
- `ui/src/pages/Settings.tsx` (add Deployment section)
- `ui/src/api/client.ts` (add system info + restart endpoints)
- `ui/src/types/index.ts` (add SystemInfo type)
