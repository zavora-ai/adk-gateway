# Requirements Document

## Introduction

Phase 2 of the adk-gateway project delivers production-grade UX polish, external coding agent integration, architectural corrections, runner configurability, improved heartbeat semantics, multi-user support, and deployment hardening. These requirements cover seven major areas that collectively bring the gateway from a working prototype to a production-ready system.

## Glossary

- **Gateway**: The adk-gateway Rust binary that bridges channels (Telegram, Slack, etc.) to adk-rust agents
- **Runner**: The adk-runner crate's execution loop that iterates tool calls until completion or limit
- **Tool_Approval_Flow**: An interactive mechanism that pauses dangerous tool execution and requests explicit user confirmation before proceeding
- **Inline_Button**: A Telegram inline keyboard button attached to a message for user interaction
- **Stale_Context_Detector**: A subsystem that detects idle periods and prompts the user with a context-refreshing welcome-back message
- **Rate_Limiter**: A subsystem that throttles rapid consecutive tool invocations to prevent runaway loops
- **ACP**: Agent Communication Protocol — the standard for delegating tasks to external coding agents (Claude Code, Codex)
- **ACP_Tool**: A delegatable tool registered in the agent's toolset that forwards execution to an external coding agent via ACP
- **Schema_Sanitizer**: A transformation layer that modifies JSON Schema tool definitions to comply with provider-specific restrictions
- **Model_Adapter**: The provider-specific implementation within adk-model that translates tool schemas and requests for a given LLM provider
- **Max_Iterations**: A configurable upper bound on the number of tool-call iterations the Runner executes per request
- **Heartbeat_V2**: The redesigned heartbeat system that runs within the user's active session with full conversation context
- **Heartbeat_Turn**: A single heartbeat invocation and its response within the session history
- **Session_History**: The ordered list of conversation turns stored for a user's session
- **Paired_User**: A user who has completed the pairing flow and is authorized to interact with the gateway
- **Channel_User**: A specific user identity within a channel (e.g., a Telegram user ID)
- **Multi_User_Channel**: A channel configuration that supports multiple paired users simultaneously
- **Thread**: A Telegram message thread or reply chain used to scope conversations in group chats
- **Agent_Router**: A subsystem that routes messages to specific agents based on group or user configuration
- **Health_Monitor**: A subsystem that periodically checks gateway health and sends alerts on degradation
- **Config_Encryption**: Encryption of sensitive configuration values (API keys, tokens) at rest
- **Zero_Downtime_Restart**: A restart mechanism that drains in-flight requests before shutting down the old process

## Requirements

### Requirement 1: Tool Approval Inline Buttons

**User Story:** As a user, I want dangerous tool calls to require my explicit approval via Telegram buttons, so that I maintain control over potentially destructive operations.

#### Acceptance Criteria

1. WHEN the Runner invokes a tool marked as `requires_approval`, THE Tool_Approval_Flow SHALL pause execution and send a Telegram message with inline buttons (✅ Approve / ❌ Reject) to the paired user
2. WHEN the user taps ✅ Approve, THE Tool_Approval_Flow SHALL resume tool execution and deliver the result to the Runner
3. WHEN the user taps ❌ Reject, THE Tool_Approval_Flow SHALL cancel the tool call and return a rejection error to the Runner
4. IF no approval response is received within 120 seconds, THEN THE Tool_Approval_Flow SHALL auto-reject the tool call and notify the user of the timeout
5. THE Gateway SHALL classify the following tool categories as `requires_approval` by default: file write operations, shell execution, and destructive filesystem operations
6. WHERE the user configures custom approval rules in the gateway config, THE Tool_Approval_Flow SHALL apply those rules instead of the defaults
7. WHILE a tool approval is pending, THE Gateway SHALL display a "⏳ Waiting for approval..." status message to the user

### Requirement 2: Stale Context Detection

**User Story:** As a user, I want the system to detect when I return after an idle period and offer a context-refreshing prompt, so that conversations resume naturally.

#### Acceptance Criteria

1. WHEN a user sends a message after an idle period exceeding the configured threshold (default: 4 hours), THE Stale_Context_Detector SHALL send a "Welcome back" prompt summarizing pending items and recent context
2. THE Stale_Context_Detector SHALL retrieve the last activity timestamp from the Session_History for idle period calculation
3. WHERE the user configures a custom idle threshold, THE Stale_Context_Detector SHALL use that threshold instead of the default
4. THE Stale_Context_Detector SHALL include in the welcome-back message: time since last interaction, count of pending scheduled task results, and any heartbeat alerts generated during the idle period
5. IF no pending items or alerts exist, THEN THE Stale_Context_Detector SHALL send a brief "Welcome back" acknowledgment without a detailed summary

### Requirement 3: Tool Loop Rate Limiting

**User Story:** As a user, I want rapid tool invocation loops to be throttled, so that runaway agent behavior is caught early and resources are preserved.

#### Acceptance Criteria

1. WHILE the Runner executes tool calls, THE Rate_Limiter SHALL track the number of tool invocations within a sliding window (default: 10 calls per 5 seconds)
2. WHEN the tool invocation rate exceeds the configured threshold, THE Rate_Limiter SHALL pause execution for a configurable cooldown period (default: 3 seconds) before resuming
3. IF the rate limit is triggered 3 times within a single request, THEN THE Rate_Limiter SHALL terminate the request and notify the user that a tool loop was detected
4. THE Rate_Limiter SHALL log each rate-limit trigger event with the tool names involved and the invocation count
5. WHERE the user configures custom rate-limit parameters, THE Rate_Limiter SHALL apply those parameters per-agent

### Requirement 4: ACP Integration for External Coding Agents

**User Story:** As a developer, I want to delegate coding tasks to Claude Code or Codex via ACP, so that the gateway agent can leverage specialized coding capabilities.

#### Acceptance Criteria

1. WHERE the `acp` feature flag is enabled, THE Gateway SHALL load the adk-acp crate and register ACP_Tools in the agent's toolset
2. THE ACP_Tool SHALL accept a task description and optional file context, forward the request to the configured external coding agent, and return the result
3. WHEN an ACP_Tool invocation is initiated, THE Gateway SHALL establish a connection to the configured ACP endpoint using the adk-acp client
4. THE Gateway SHALL support configuring ACP agents per-agent in the gateway config, allowing different specialist agents to delegate to different coding agents
5. IF the ACP endpoint is unreachable or returns an error, THEN THE ACP_Tool SHALL return a descriptive error message to the Runner without crashing
6. THE ACP_Tool SHALL support a configurable timeout (default: 300 seconds) for long-running coding tasks
7. WHILE an ACP task is executing, THE Gateway SHALL send periodic progress messages to the user indicating the task is in progress

### Requirement 5: Provider-Aware Schema Sanitization

**User Story:** As a developer, I want JSON Schema sanitization to be applied only for providers that need it (Gemini), so that other providers receive unmodified schemas and function correctly.

#### Acceptance Criteria

1. THE Model_Adapter for Gemini SHALL apply schema sanitization transformations before sending tool definitions to the Gemini API
2. THE Model_Adapter for Gemini SHALL remove or transform the following unsupported schema properties: `exclusiveMinimum`, `exclusiveMaximum`, `items` when it is an array, `propertyNames`, and `type` when it is an array
3. THE Model_Adapter for OpenAI SHALL pass tool JSON Schemas unmodified to the OpenAI API
4. THE Model_Adapter for Anthropic SHALL pass tool JSON Schemas unmodified to the Anthropic API
5. THE Gateway SHALL remove any provider-agnostic schema sanitization currently applied in the MCP toolset layer
6. WHEN a new MCP tool is discovered, THE Gateway SHALL store the original unmodified schema and apply provider-specific transformations only at model invocation time
7. THE Schema_Sanitizer for Gemini SHALL convert `exclusiveMinimum: N` to `minimum: N+1` and `exclusiveMaximum: N` to `maximum: N-1` for integer types
8. THE Schema_Sanitizer for Gemini SHALL convert array-typed `type` fields (e.g., `["string", "null"]`) to a single type with `nullable: true`

### Requirement 6: Runner Max Iterations Configuration

**User Story:** As a gateway operator, I want to configure the maximum number of tool-call iterations per request, so that I can prevent runaway loops and control resource usage.

#### Acceptance Criteria

1. THE Runner SHALL accept a configurable `max_iterations` parameter (default: 100) that limits the number of tool-call loop iterations per request
2. WHEN the iteration count reaches `max_iterations`, THE Runner SHALL terminate the loop and return a partial result with a max-iterations-reached indicator
3. THE Gateway SHALL support setting a per-request `max_iterations` value that overrides the Runner default (e.g., 25 for gateway requests)
4. WHERE the gateway config specifies a `max_iterations` value, THE Gateway SHALL pass that value to the Runner for every request
5. IF `max_iterations` is set to a value less than 1 or greater than 1000, THEN THE Gateway SHALL reject the configuration with a validation error
6. THE Runner SHALL include the iteration count in its response metadata so the Gateway can report it to the user

### Requirement 7: Heartbeat V2 — Runner-Level Integration

**User Story:** As a user, I want the heartbeat to run within my active session with full conversation context, so that heartbeat responses are contextually aware and alerts are meaningful.

#### Acceptance Criteria

1. THE Heartbeat_V2 SHALL execute within the user's active session, with access to the full Session_History as context
2. WHEN the heartbeat fires, THE Heartbeat_V2 SHALL inject a heartbeat prompt into the session and process it through the Runner
3. THE Heartbeat_V2 SHALL strip Heartbeat_Turns from the Session_History before persisting, retaining only turns that produced actionable alerts
4. WHEN the heartbeat response is exactly "HEARTBEAT_OK", THE Heartbeat_V2 SHALL discard both the prompt and response from Session_History
5. WHEN the heartbeat response contains an actionable alert, THE Heartbeat_V2 SHALL retain the response in Session_History and deliver the alert to the user
6. THE Heartbeat_V2 SHALL replace the current cron-job-based heartbeat implementation entirely
7. THE Heartbeat_V2 SHALL support per-user heartbeat scheduling, firing independently for each Paired_User
8. IF the user's session is not active (no session exists), THEN THE Heartbeat_V2 SHALL create a temporary session for the heartbeat check and discard it after completion

### Requirement 8: Multi-User Channel Support

**User Story:** As a team, we want multiple users to pair with the same gateway channel, so that each team member gets personalized responses and heartbeat delivery.

#### Acceptance Criteria

1. THE Gateway SHALL support multiple Paired_Users per channel, each identified by their Channel_User ID
2. WHEN a new user sends the pairing code, THE Gateway SHALL register them as an additional Paired_User without affecting existing pairings
3. THE Heartbeat_V2 SHALL deliver alerts to each Paired_User independently based on their individual session context
4. WHEN a message arrives in a group chat, THE Gateway SHALL use the Thread context to scope the response to the correct conversation thread
5. WHERE per-group agent routing is configured, THE Agent_Router SHALL route messages from specific groups to designated agents
6. THE Gateway SHALL maintain separate Session_History per Paired_User, even within the same channel
7. IF a Paired_User is removed (unpaired), THEN THE Gateway SHALL stop heartbeat delivery and session tracking for that user without affecting other paired users

### Requirement 9: Docker Deployment

**User Story:** As a DevOps engineer, I want a production-ready Docker image with multi-stage build, so that deployment is reproducible and minimal.

#### Acceptance Criteria

1. THE Gateway SHALL provide a Dockerfile with a multi-stage build: a Rust builder stage and a minimal runtime stage (distroless or alpine)
2. THE Dockerfile SHALL produce a final image smaller than 100MB (excluding mounted volumes)
3. THE Dockerfile SHALL support build arguments for feature flags (e.g., `--features browser,postgres`)
4. WHEN the container starts, THE Gateway SHALL read configuration from environment variables and/or a mounted config file
5. THE Dockerfile SHALL include a HEALTHCHECK instruction that calls the `/health` endpoint

### Requirement 10: Systemd Service Integration

**User Story:** As a system administrator, I want a systemd service file for the gateway, so that it starts on boot, restarts on failure, and integrates with system logging.

#### Acceptance Criteria

1. THE Gateway SHALL provide a systemd service unit file (`adk-gateway.service`) with `Type=notify` for readiness signaling
2. THE service file SHALL configure automatic restart on failure with a 5-second delay
3. THE service file SHALL set resource limits (memory, file descriptors) appropriate for production use
4. THE Gateway SHALL send the `sd_notify(READY=1)` signal when it has completed initialization and is accepting connections
5. WHEN a graceful shutdown signal (SIGTERM) is received, THE Gateway SHALL drain in-flight requests within a configurable timeout (default: 30 seconds) before exiting

### Requirement 11: Health Monitoring with Alerting

**User Story:** As an operator, I want automated health monitoring with alerting, so that I am notified when the gateway degrades.

#### Acceptance Criteria

1. THE Health_Monitor SHALL periodically check gateway health (default: every 60 seconds) including channel connectivity, model reachability, and session store availability
2. WHEN a health check fails for 3 consecutive attempts, THE Health_Monitor SHALL emit an alert via the configured alerting channel (webhook, Telegram message to admin, or log)
3. WHEN a previously failing component recovers, THE Health_Monitor SHALL emit a recovery notification
4. THE Health_Monitor SHALL expose health status via the existing `/health` endpoint with per-component breakdown
5. WHERE alerting is configured with a webhook URL, THE Health_Monitor SHALL POST a JSON payload with component name, status, failure count, and timestamp

### Requirement 12: Structured Log Rotation

**User Story:** As an operator, I want structured log rotation with configurable retention, so that disk space is managed automatically.

#### Acceptance Criteria

1. THE Gateway SHALL output structured JSON logs to rotating log files using the existing tracing-appender infrastructure
2. THE Gateway SHALL support configurable log rotation: daily rotation with a configurable retention period (default: 7 days)
3. WHEN the retention period is exceeded, THE Gateway SHALL delete log files older than the configured threshold
4. THE Gateway SHALL support configurable maximum log file size with rotation on size threshold (default: 100MB per file)
5. WHERE the `LOG_FORMAT` environment variable is set to `pretty`, THE Gateway SHALL output human-readable logs instead of JSON

### Requirement 13: Graceful Zero-Downtime Restarts

**User Story:** As an operator, I want zero-downtime restarts, so that in-flight requests complete before the old process exits.

#### Acceptance Criteria

1. WHEN a restart signal (SIGUSR1) is received, THE Gateway SHALL stop accepting new connections while continuing to process in-flight requests
2. WHEN all in-flight requests complete or the drain timeout (default: 30 seconds) expires, THE Gateway SHALL exit with code 0
3. THE Gateway SHALL support socket-based handoff: the new process binds to the same port before the old process releases it
4. IF in-flight requests do not complete within the drain timeout, THEN THE Gateway SHALL forcefully terminate remaining requests and log a warning with request details
5. THE Gateway SHALL emit a structured log event at each phase of the restart: drain-start, drain-complete, and shutdown

### Requirement 14: Config Encryption for Secrets

**User Story:** As a security-conscious operator, I want sensitive configuration values encrypted at rest, so that API keys and tokens are not stored in plaintext.

#### Acceptance Criteria

1. THE Gateway SHALL support encrypting sensitive configuration fields (API keys, bot tokens, client secrets) using AES-256-GCM
2. WHEN the gateway starts with an encryption key configured, THE Gateway SHALL decrypt encrypted config values before use
3. THE Gateway SHALL provide a CLI command (`adk-gateway config encrypt`) that encrypts plaintext secrets in the config file in-place
4. THE Gateway SHALL identify sensitive fields by convention: any field name containing `key`, `token`, `secret`, or `password`
5. IF the encryption key is not available at startup and encrypted values are present, THEN THE Gateway SHALL exit with a clear error message indicating the missing decryption key
6. THE Gateway SHALL store encrypted values with a recognizable prefix (e.g., `enc:`) so that plaintext and encrypted values can coexist during migration
