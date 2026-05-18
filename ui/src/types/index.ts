// API response envelope — matches Rust ApiResponse<T>
export interface ApiResponse<T = unknown> {
  ok: boolean;
  data?: T;
  message?: string;
}

// Dashboard
export interface DashboardData {
  uptime_secs: number;
  connected_channels: ChannelInfo[];
  active_session_count: number;
  memory_status: SubsystemStatus | null;
  rag_status: SubsystemStatus | null;
}

export interface ChannelInfo {
  channel_type: string;
  account_id: string;
  status: string;
}

export interface SubsystemStatus {
  backend_type: string;
  healthy: boolean;
  details: string;
}

// Sessions
export interface SessionInfo {
  session_id: string;
  user_id: string;
  channel_type: string;
  last_activity: string;
}

// Logs
export interface LogEntry {
  timestamp: string;
  level: string;
  message: string;
  target: string | null;
}

// Agents
export interface AgentRecord {
  id: string;
  name: string;
  description: string;
  agent_type: string;
  state: string;
  port: number | null;
  model: string;
  tools: string[];
  instruction: string;
  api_key_env: string;
  auto_start: boolean;
  channel_bindings: { channel_type: string; account_id: string | null }[];
  created_at: string;
  updated_at: string;
}

// AWP
// AWP — matches actual API response shape from /ui/api/awp
export interface AwpSummary {
  health: {
    state: string;
    message: string;
    timestamp: string;
  };
  site: {
    name: string;
    description: string;
    domain: string;
  };
  capability_count: number;
}

export interface AwpCapability {
  name: string;
  description: string;
  endpoint: string;
  method: string;
  access_level: string;
}

export interface AwpSubscription {
  id: string;
  subscriber: string;
  callback_url: string;
  event_types: string[];
}

export interface AwpHealthState {
  state: string;
  message: string;
  timestamp: string;
}

// Integrations
export interface McpServerInfo {
  server_id: string;
  transport?: string;
  transport_detail?: {
    command?: string;
    args?: string[];
    env?: Record<string, string>;
    url?: string;
  };
  status: string;
  enabled?: boolean;
  tools?: string[];
  discovered_tools?: string[];
}

export interface CronJobInfo {
  id: string;
  schedule: string;
  message: string;
  delivery: { channel: string; target: string } | null;
  status: string;
  last_error?: { message: string; timestamp: string } | null;
  suppress_keyword?: string | null;
}

export interface ToolInfo {
  name: string;
  description: string;
  source: string;
}

// Settings
export interface SettingsResponse {
  ok: boolean;
  message: string;
}

// Auth
export interface AuthStatus {
  authenticated: boolean;
  mode: string;
}

// Agent Category Config — categories are now arrays (fallback chains)
export interface AgentCategoryConfig {
  primary: string;
  vision: string[] | null;
  omni: string[] | null;
  image_generation: string[] | null;
  tts: string[] | null;
  stt: string[] | null;
  code: string[] | null;
  embedding: string[] | null;
  search: string[] | null;
  music: string[] | null;
  cloud_provider?: {
    type: string;
    [key: string]: string;
  };
}

// WebSocket events
export type WsEvent =
  | { type: 'connected'; message: string }
  | { type: 'log'; timestamp: string; level: string; message: string; target?: string }
  | { type: 'agent_state'; agent_id: string; state: string }
  | { type: 'dashboard'; uptime_secs: number; session_count: number; channel_count: number }
  | { type: 'coding_agent_status'; agent_id: string; status: CodingAgentConnectionStatus; timestamp: string }
  | { type: 'coding_agent_task'; agent_id: string; task: TaskHistoryEntry }
  | { type: 'coding_agent_cost_warning'; agent_id: string; threshold_usd: number; current_usd: number };

// Delegation
export interface DelegationRule {
  caller_id: string;
  target_id: string;
  created_at: string;
}

// --- Phase 2 Types ---

// Tool Approval (Task 14)
export interface PendingApproval {
  id: string;
  tool_name: string;
  tool_args: Record<string, unknown>;
  user_id: string;
  requested_at: string;
  expires_at: string;
  state: 'pending' | 'approved' | 'rejected' | 'timed_out';
}

export interface ApprovalConfig {
  enabled: boolean;
  require_approval: string[];
  timeout_secs: number;
}

export interface ApprovalHistoryEntry {
  id: string;
  tool_name: string;
  user_id: string;
  state: string;
  timestamp: string;
  resolved_at?: string;
}

// Stale Context (Task 15)
export interface StaleContextConfig {
  idle_threshold_hours: number;
}

export interface UserActivity {
  user_id: string;
  channel: string;
  last_active: string;
  idle_hours: number;
}

// Rate Limiter (Task 16)
export interface RateLimitConfig {
  max_calls: number;
  window_secs: number;
  cooldown_secs: number;
  max_triggers: number;
}

export interface RateLimitMetrics {
  triggered_today: number;
}

// ACP Integration (Task 17)
export interface AcpAgentInfo {
  name: string;
  command: string;
  working_directory?: string;
  auto_approve: boolean;
  timeout_secs: number;
  status: 'connected' | 'disconnected' | 'error';
}

export interface AcpAgentForm {
  name: string;
  command: string;
  working_directory: string;
  auto_approve: boolean;
  timeout_secs: number;
}

// Health Monitor (Task 18)
export interface HealthComponent {
  name: string;
  status: 'healthy' | 'degraded' | 'unhealthy';
  consecutive_failures: number;
  last_check: string;
  latency_ms?: number;
}

export interface HealthEvent {
  timestamp: string;
  component: string;
  event: 'alert' | 'recovery';
  message: string;
}

export interface HealthMonitorConfig {
  check_interval_secs: number;
  failure_threshold: number;
  alert_webhook_url: string;
  alert_telegram_admin: string;
}

// Multi-User (Task 19)
export interface PairedUser {
  user_id: string;
  channel: string;
  paired_at: string;
  last_active: string;
  heartbeat_enabled: boolean;
  heartbeat_interval_secs?: number;
  heartbeat_status: 'active' | 'paused' | 'disabled';
}

export interface GroupChatAssignment {
  group_id: string;
  thread_id?: string;
  agent_id: string;
}

// Config Encryption (Task 20)
export interface EncryptionStatus {
  enabled: boolean;
  key_configured: boolean;
  key_file_path?: string;
}

export interface SensitiveField {
  field_name: string;
  encrypted: boolean;
}

// Log Rotation (Task 21)
export interface LogRotationConfig {
  rotation_policy: 'daily' | 'hourly' | 'size';
  retention_days: number;
  max_file_size_mb: number;
  format: 'json' | 'pretty';
}

export interface LogStorageMetrics {
  total_size_bytes: number;
  file_count: number;
  oldest_file_date: string;
}

export interface LogFileInfo {
  filename: string;
  size_bytes: number;
  created_at: string;
}

// Deployment Status (Task 22)
export interface SystemInfo {
  version: string;
  uptime_secs: number;
  build_features: string[];
  config_path: string;
  docker_status?: string;
  systemd_status?: string;
  drain_timeout_secs: number;
}

export interface RestartStatus {
  restarting: boolean;
  in_flight_requests: number;
  phase: 'idle' | 'drain-start' | 'drain-complete' | 'shutdown';
}

// --- Coding Agent Types ---

/** Authentication method variants. */
export type AgentAuthMethod =
  | { type: 'apiKey'; env_var: string }
  | { type: 'oAuth'; auth_url: string; token_url: string }
  | { type: 'cliLogin'; command: string }
  | { type: 'none' };

/** Capabilities declared by a backend. */
export interface AgentCapabilities {
  file_context: boolean;
  streaming_output: boolean;
  cost_reporting: boolean;
  cancellation: boolean;
}

/** Connection status for a coding agent. */
export type CodingAgentConnectionStatus =
  | 'connected'
  | 'disconnected'
  | 'error';

/** Task trigger source. */
export type TaskTrigger =
  | { type: 'userCommand'; user_id: string; channel: string }
  | { type: 'cronJob'; job_id: string }
  | { type: 'agentDelegation'; source_agent_id: string }
  | { type: 'controlPanel'; user_id: string };

/** Task outcome status. */
export type TaskOutcome = 'success' | 'failure' | 'timeout' | 'cancelled';

/** File change in a task result. */
export interface FileChange {
  path: string;
  change_type: 'added' | 'modified' | 'deleted';
  lines_added: number;
  lines_removed: number;
}

/** Token usage breakdown. */
export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  estimated_cost_usd: number;
}

/** Task error detail. */
export type TaskError =
  | { category: 'timeout'; elapsed_secs: number; limit_secs: number }
  | { category: 'costCap'; spent_usd: number; cap_usd: number }
  | { category: 'rateLimit'; retry_after_secs: number | null }
  | { category: 'executionError'; message: string; partial_output: string | null }
  | { category: 'agentDisconnected'; agent_id: string };

/** Backend type definition for a coding agent CLI. */
export interface CodingAgentBackend {
  agent_type: string;
  display_name: string;
  cli_command: string;
  install_check_command: string;
  auth_method: AgentAuthMethod;
  capabilities: AgentCapabilities;
  install_instructions: string;
}

/** A registered coding agent summary (list view). */
export interface CodingAgentSummary {
  id: string;
  alias: string | null;
  backend_type: string;
  display_name: string;
  connection_status: CodingAgentConnectionStatus;
  last_task_at: string | null;
  workspaces: string[];
}

/** Full coding agent detail. */
export interface CodingAgentDetail extends CodingAgentSummary {
  endpoint: string;
  timeout_secs: number;
  cost_cap_usd: number | null;
  monthly_budget_usd: number | null;
  capabilities: AgentCapabilities;
}

/** Task history entry (table row). */
export interface TaskHistoryEntry {
  task_id: string;
  agent_id: string;
  description: string;
  trigger: TaskTrigger;
  outcome: TaskOutcome;
  started_at: string;
  duration_ms: number;
  created_at: string;
}

/** Full task detail. */
export interface TaskDetail extends TaskHistoryEntry {
  completed_at: string | null;
  output: string;
  modified_files: FileChange[];
  token_usage: TokenUsage | null;
  error: TaskError | null;
  workspace: string;
}

/** Cost statistics for an agent. */
export interface AgentCostStats {
  agent_id: string;
  total_input_tokens: number;
  total_output_tokens: number;
  estimated_total_cost_usd: number;
  task_count: number;
  period_start: string;
  period_end: string;
}

/** Agent configuration update payload. */
export interface AgentConfigUpdate {
  cost_cap_usd: number | null;
  timeout_secs: number;
  workspaces: string[];
}

/** Task delegation request payload. */
export interface TaskDelegationPayload {
  description: string;
  workspace: string | null;
  file_context: string[] | null;
}

/** Onboarding registration payload. */
export interface AgentRegistrationPayload {
  backend_type: string;
  alias: string;
  endpoint: string;
  workspaces: string[];
  timeout_secs: number | null;
  cost_cap_usd: number | null;
  auth: { credentials?: string; token?: string } | null;
}

/** CLI verification result. */
export interface CliVerificationResult {
  installed: boolean;
  version: string | null;
  path: string | null;
}

/** Paginated response wrapper. */
export interface PaginatedResponse<T> {
  items: T[];
  total: number;
  page: number;
  page_size: number;
}

/** WebSocket events for coding agents. */
export type CodingAgentWsEvent =
  | { type: 'coding_agent_status'; agent_id: string; status: CodingAgentConnectionStatus; timestamp: string }
  | { type: 'coding_agent_task'; agent_id: string; task: TaskHistoryEntry }
  | { type: 'coding_agent_cost_warning'; agent_id: string; threshold_usd: number; current_usd: number };
