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
  | { type: 'dashboard'; uptime_secs: number; session_count: number; channel_count: number };

// Delegation
export interface DelegationRule {
  caller_id: string;
  target_id: string;
  created_at: string;
}
