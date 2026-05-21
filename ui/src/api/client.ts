import type {
  ApiResponse,
  CodingAgentSummary,
  CodingAgentDetail,
  AgentRegistrationPayload,
  AgentConfigUpdate,
  PaginatedResponse,
  TaskHistoryEntry,
  TaskDetail,
  TaskDelegationPayload,
  AgentCostStats,
  CodingAgentBackend,
  CliVerificationResult,
} from '../types';

const BASE = '/ui/api';

/** Normalize a backend status value to a valid CodingAgentConnectionStatus. */
function normalizeStatus(status: unknown): 'connected' | 'disconnected' | 'error' {
  if (status == null) return 'disconnected';
  // Handle string values
  if (typeof status === 'string') {
    const s = status.toLowerCase();
    if (s === 'connected' || s === 'running' || s === 'active') return 'connected';
    if (s === 'error' || s === 'failed') return 'error';
    if (s === 'disconnected') return 'disconnected';
    return 'disconnected'; // "unknown" and others
  }
  // Handle object variants: {"disconnected": {...}} or {"error": {...}}
  if (typeof status === 'object') {
    if ('connected' in (status as object)) return 'connected';
    if ('error' in (status as object)) return 'error';
    if ('disconnected' in (status as object)) return 'disconnected';
  }
  return 'disconnected';
}

/** Extract error message from status object if present. */
function extractStatusMessage(status: unknown): string | null {
  if (status == null || typeof status !== 'object') return null;
  const obj = status as Record<string, unknown>;
  if ('error' in obj && typeof obj.error === 'object' && obj.error !== null) {
    return (obj.error as Record<string, unknown>).message as string ?? null;
  }
  if ('disconnected' in obj && typeof obj.disconnected === 'object' && obj.disconnected !== null) {
    const since = (obj.disconnected as Record<string, unknown>).since as string;
    return since ? `Since ${new Date(since).toLocaleString()}` : null;
  }
  return null;
}

/** Typed fetch wrapper for the gateway JSON API. */
async function request<T>(
  path: string,
  options: RequestInit = {},
): Promise<ApiResponse<T>> {
  const url = `${BASE}${path}`;
  const res = await fetch(url, {
    credentials: 'same-origin',
    headers: {
      'Content-Type': 'application/json',
      ...((options.headers as Record<string, string>) || {}),
    },
    ...options,
  });

  if (res.status === 401) {
    // Redirect to login on auth failure
    window.location.href = '/ui/login';
    return { ok: false, message: 'Authentication required' };
  }

  if (res.status === 204) {
    return { ok: true };
  }

  const json = await res.json();

  // Normalize: some endpoints return raw data, others use ApiResponse envelope
  if (json.ok !== undefined) {
    return json as ApiResponse<T>;
  }
  return { ok: true, data: json as T };
}

export const api = {
  get: <T>(path: string) => request<T>(path),

  post: <T>(path: string, body?: unknown) =>
    request<T>(path, {
      method: 'POST',
      body: body ? JSON.stringify(body) : undefined,
    }),

  del: <T>(path: string) =>
    request<T>(path, { method: 'DELETE' }),

  // Auth
  checkAuth: () => request<{ authenticated: boolean; mode: string }>('/auth/check'),
  login: (password: string) => request<void>('/login', { method: 'POST', body: JSON.stringify({ password }) }),
  logout: () => request<void>('/logout', { method: 'POST' }),

  // Dashboard
  dashboard: () => request<import('../types').DashboardData>('/dashboard'),

  // Sessions
  sessions: () => request<import('../types').SessionInfo[]>('/sessions'),
  terminateSession: (id: string) => request<void>(`/sessions/${encodeURIComponent(id)}/terminate`, { method: 'POST' }),

  // Config
  config: () => request<Record<string, unknown>>('/config'),
  saveConfig: (config: string) => request<void>('/config', { method: 'POST', body: JSON.stringify({ config }) }),

  // Logs
  logs: () => request<import('../types').LogEntry[]>('/logs'),

  // Settings
  saveSettings: (settings: unknown) => request<void>('/settings', { method: 'POST', body: JSON.stringify(settings) }),
  sessionStatus: () => request<{ backend: string; healthy: boolean; connection_string: string }>('/settings/session-status'),

  // Channels
  getChannels: () => request<{
    telegram?: { enabled: boolean; bot_token: string; dm_policy: string; stream_mode: string };
    slack?: { enabled: boolean; bot_token: string; app_token: string; dm_policy: string };
    whatsapp?: { enabled: boolean; phone_number_id: string; access_token: string; verify_token: string; webhook_path: string };
    discord?: { enabled: boolean; bot_token: string; application_id: string; guild_ids: string[] };
    matrix?: { enabled: boolean; homeserver_url: string; access_token: string; user_id: string; room_ids: string[] };
  }>('/channels'),
  saveChannels: (channels: unknown) => request<void>('/channels', { method: 'POST', body: JSON.stringify(channels) }),
  probeTelegram: () => request<{ status: string; bot_username?: string }>('/channels/telegram/probe', { method: 'POST' }),

  // Agent & Model
  getAgent: () => request<import('../types').AgentCategoryConfig>('/agent'),
  saveAgent: (agent: unknown) => request<void>('/agent', { method: 'POST', body: JSON.stringify(agent) }),

  // Memory
  loadMemory: () => request<{ content: string; path: string; exists: boolean }>('/memory'),
  saveMemory: (content: string) => request<void>('/memory', { method: 'POST', body: JSON.stringify({ content }) }),
  memoryEntities: () => request<{ users: Array<{ user_id: string; entity_count: number; relation_count: number; entities: Array<{ name: string; entity_type: string; observations: string[] }>; relations: Array<{ source: string; relation_type: string; target: string }> }> }>('/memory/entities'),

  // Agents
  agents: () => request<import('../types').AgentRecord[]>('/agents'),
  createAgent: (agent: unknown) => request<void>('/agents', { method: 'POST', body: JSON.stringify(agent) }),
  startAgent: (id: string) => request<void>(`/agents/${encodeURIComponent(id)}/start`, { method: 'POST' }),
  stopAgent: (id: string) => request<void>(`/agents/${encodeURIComponent(id)}/stop`, { method: 'POST' }),
  deleteAgent: (id: string) => request<void>(`/agents/${encodeURIComponent(id)}/delete`, { method: 'POST' }),
  agentLogs: (id: string) => request<{ logs: import('../types').LogEntry[] }>(`/agents/${encodeURIComponent(id)}/logs`),
  configureAgent: (id: string, config: unknown) => request<void>(`/agents/${encodeURIComponent(id)}/configure`, { method: 'POST', body: JSON.stringify(config) }),

  // AWP
  awpSummary: () => request<import('../types').AwpSummary>('/awp'),
  awpHealth: () => request<import('../types').AwpHealthState>('/awp/health'),
  awpCapabilities: () => request<import('../types').AwpCapability[]>('/awp/capabilities'),
  awpSubscriptions: () => request<import('../types').AwpSubscription[]>('/awp/subscriptions'),
  deleteAwpSubscription: (id: string) => api.del(`/awp/subscriptions/${id}`),
  awpConsent: () => request<unknown[]>('/awp/consent'),

  // Integrations
  mcpServers: () => request<import('../types').McpServerInfo[]>('/integrations/mcp'),
  addMcpServer: (server: unknown) => request<void>('/integrations/mcp', { method: 'POST', body: JSON.stringify(server) }),
  removeMcpServer: (id: string) => request<void>(`/integrations/mcp/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  toggleMcpServer: (id: string) => request<void>(`/integrations/mcp/${encodeURIComponent(id)}/toggle`, { method: 'POST' }),
  cronJobs: () => request<{ jobs: import('../types').CronJobInfo[]; total: number }>('/integrations/cron'),
  createScheduledTask: (task: { id: string; schedule: string; message: string; delivery?: { channel: string; target: string }; suppress_keyword?: string }) =>
    request<void>('/scheduled-tasks', { method: 'POST', body: JSON.stringify(task) }),
  cancelScheduledTask: (id: string) => request<void>(`/scheduled-tasks/${encodeURIComponent(id)}/cancel`, { method: 'POST' }),
  resumeScheduledTask: (id: string) => request<void>(`/scheduled-tasks/${encodeURIComponent(id)}/resume`, { method: 'POST' }),
  deleteScheduledTask: (id: string) => request<void>(`/scheduled-tasks/${encodeURIComponent(id)}`, { method: 'DELETE' }),
  scheduledTaskLogs: (id: string) => request<{ task_id: string; logs: Array<{ id: number; task_id: string; timestamp: string; event_type: string; message: string }>; count: number }>(`/scheduled-tasks/${encodeURIComponent(id)}/logs`),
  tools: () => request<{ tools: import('../types').ToolInfo[]; total: number }>('/integrations/tools'),

  // Delegation
  delegationList: () => request<import('../types').DelegationRule[]>('/delegation'),
  delegationAdd: (caller_id: string, target_id: string) =>
    request<void>('/delegation', { method: 'POST', body: JSON.stringify({ caller_id, target_id }) }),
  delegationRemove: (caller_id: string, target_id: string) =>
    request<void>('/delegation', { method: 'DELETE', body: JSON.stringify({ caller_id, target_id }) }),

  // --- Phase 2 Endpoints ---

  // Tool Approval (Task 14)
  pendingApprovals: () => request<import('../types').PendingApproval[]>('/approvals/pending'),
  approvalHistory: () => request<import('../types').ApprovalHistoryEntry[]>('/approvals/history'),
  approveRequest: (id: string) => request<void>(`/approvals/${encodeURIComponent(id)}/approve`, { method: 'POST' }),
  rejectRequest: (id: string) => request<void>(`/approvals/${encodeURIComponent(id)}/reject`, { method: 'POST' }),
  getApprovalConfig: () => request<import('../types').ApprovalConfig>('/approvals/config'),
  saveApprovalConfig: (config: import('../types').ApprovalConfig) =>
    request<void>('/approvals/config', { method: 'POST', body: JSON.stringify(config) }),

  // Stale Context (Task 15)
  getStaleContextConfig: () => request<import('../types').StaleContextConfig>('/settings/stale-context'),
  saveStaleContextConfig: (config: import('../types').StaleContextConfig) =>
    request<void>('/settings/stale-context', { method: 'POST', body: JSON.stringify(config) }),
  getUserActivities: () => request<import('../types').UserActivity[]>('/users/activity'),

  // Rate Limiter (Task 16)
  getRateLimitConfig: () => request<import('../types').RateLimitConfig>('/settings/rate-limit'),
  saveRateLimitConfig: (config: import('../types').RateLimitConfig) =>
    request<void>('/settings/rate-limit', { method: 'POST', body: JSON.stringify(config) }),
  getRateLimitMetrics: () => request<import('../types').RateLimitMetrics>('/metrics/rate-limit'),

  // ACP Integration (Task 17)
  getAcpAgents: () => request<import('../types').AcpAgentInfo[]>('/integrations/acp'),
  addAcpAgent: (agent: import('../types').AcpAgentForm) =>
    request<void>('/integrations/acp', { method: 'POST', body: JSON.stringify(agent) }),
  removeAcpAgent: (name: string) =>
    request<void>(`/integrations/acp/${encodeURIComponent(name)}`, { method: 'DELETE' }),
  getAcpFeatureEnabled: () => request<{ enabled: boolean }>('/integrations/acp/status'),

  // Health Monitor (Task 18)
  getHealthComponents: () => request<import('../types').HealthComponent[]>('/health/components'),
  getHealthEvents: () => request<import('../types').HealthEvent[]>('/health/events'),
  getHealthMonitorConfig: () => request<import('../types').HealthMonitorConfig>('/settings/health-monitor'),
  saveHealthMonitorConfig: (config: import('../types').HealthMonitorConfig) =>
    request<void>('/settings/health-monitor', { method: 'POST', body: JSON.stringify(config) }),

  // Multi-User (Task 19)
  getPairedUsers: () => request<import('../types').PairedUser[]>('/users/paired'),
  unpairUser: (userId: string) =>
    request<void>(`/users/paired/${encodeURIComponent(userId)}/unpair`, { method: 'POST' }),
  updateUserHeartbeat: (userId: string, config: { enabled: boolean; interval_secs?: number }) =>
    request<void>(`/users/paired/${encodeURIComponent(userId)}/heartbeat`, { method: 'POST', body: JSON.stringify(config) }),
  getGroupAssignments: () => request<import('../types').GroupChatAssignment[]>('/users/groups'),
  saveGroupAssignment: (assignment: import('../types').GroupChatAssignment) =>
    request<void>('/users/groups', { method: 'POST', body: JSON.stringify(assignment) }),

  // Config Encryption (Task 20)
  getEncryptionStatus: () => request<import('../types').EncryptionStatus>('/settings/encryption/status'),
  getSensitiveFields: () => request<import('../types').SensitiveField[]>('/settings/encryption/fields'),
  encryptAll: () => request<void>('/settings/encryption/encrypt-all', { method: 'POST' }),
  saveEncryptionKeyPath: (path: string) =>
    request<void>('/settings/encryption/key-path', { method: 'POST', body: JSON.stringify({ path }) }),

  // Log Rotation (Task 21)
  getLogRotationConfig: () => request<import('../types').LogRotationConfig>('/settings/log-rotation'),
  saveLogRotationConfig: (config: import('../types').LogRotationConfig) =>
    request<void>('/settings/log-rotation', { method: 'POST', body: JSON.stringify(config) }),
  getLogStorageMetrics: () => request<import('../types').LogStorageMetrics>('/metrics/log-storage'),
  getLogFiles: () => request<import('../types').LogFileInfo[]>('/logs/files'),
  downloadLogFile: (filename: string) => `${BASE}/logs/files/${encodeURIComponent(filename)}/download`,
  clearOldLogs: () => request<void>('/logs/clear-old', { method: 'POST' }),

  // Deployment Status (Task 22)
  getSystemInfo: () => request<import('../types').SystemInfo>('/system/info'),
  getRestartStatus: () => request<import('../types').RestartStatus>('/system/restart/status'),
  triggerRestart: () => request<void>('/system/restart', { method: 'POST' }),

  // --- Coding Agents ---
  codingAgents: async (): Promise<ApiResponse<CodingAgentSummary[]>> => {
    const res = await request<unknown>('/coding-agents');
    const raw = res as unknown as { ok: boolean; data?: Array<Record<string, unknown>>; message?: string };
    if (raw.ok && raw.data) {
      // Normalize backend response to match CodingAgentSummary interface
      const agents: CodingAgentSummary[] = raw.data.map((a) => ({
        id: (a.id as string) ?? '',
        alias: (a.alias as string) ?? null,
        backend_type: (a.backendType as string) ?? (a.backend_type as string) ?? '',
        display_name: (a.displayName as string) ?? (a.display_name as string) ?? (a.backendType as string) ?? '',
        connection_status: normalizeStatus(a.status),
        status_message: extractStatusMessage(a.status),
        last_task_at: (a.lastSuccessfulTask as string) ?? (a.last_task_at as string) ?? null,
        workspaces: (a.workspaces as string[]) ?? [],
      }));
      return { ok: true, data: agents };
    }
    return { ok: false, message: raw.message || 'Failed to load agents' };
  },
  codingAgent: async (id: string): Promise<ApiResponse<CodingAgentDetail>> => {
    const res = await request<unknown>(`/coding-agents/${encodeURIComponent(id)}`);
    const raw = res as unknown as { ok: boolean; data?: Record<string, unknown>; message?: string };
    if (raw.ok && raw.data) {
      const a = raw.data;
      const detail: CodingAgentDetail = {
        id: (a.id as string) ?? '',
        alias: (a.alias as string) ?? null,
        backend_type: (a.backendType as string) ?? (a.backend_type as string) ?? '',
        display_name: (a.displayName as string) ?? (a.display_name as string) ?? (a.backendType as string) ?? '',
        connection_status: normalizeStatus(a.status),
        status_message: extractStatusMessage(a.status),
        last_task_at: (a.lastSuccessfulTask as string) ?? (a.last_task_at as string) ?? null,
        workspaces: (a.workspaces as string[]) ?? [],
        endpoint: (a.endpoint as string) ?? '',
        timeout_secs: (a.timeoutSecs as number) ?? (a.timeout_secs as number) ?? 1800,
        cost_cap_usd: (a.costCapUsd as number | null) ?? (a.cost_cap_usd as number | null) ?? null,
        monthly_budget_usd: (a.monthlyBudgetUsd as number | null) ?? (a.monthly_budget_usd as number | null) ?? null,
        capabilities: (a.capabilities as CodingAgentDetail['capabilities']) ?? { file_context: false, streaming_output: false, cost_reporting: false, cancellation: false },
      };
      return { ok: true, data: detail };
    }
    return { ok: false, message: raw.message || 'Failed to load agent' };
  },
  registerCodingAgent: async (payload: AgentRegistrationPayload, transport?: { type: string; command: string; args: string[]; env: Record<string, string> }): Promise<ApiResponse<CodingAgentSummary>> => {
    // Transform to match backend's expected camelCase format with required 'id' field
    const id = payload.alias || `${payload.backend_type}-${Date.now()}`;
    const body: Record<string, unknown> = {
      id,
      backendType: payload.backend_type,
      endpoint: payload.endpoint || `acp://${payload.backend_type}`,
      workspaces: payload.workspaces,
      timeoutSecs: payload.timeout_secs,
      costCapUsd: payload.cost_cap_usd,
      monthlyBudgetUsd: null,
      alias: payload.alias || null,
    };
    if (transport) {
      body.transport = transport;
    }
    if (payload.auth?.credentials) {
      body.auth = { credentials: payload.auth.credentials };
    }
    return request<CodingAgentSummary>('/coding-agents', { method: 'POST', body: JSON.stringify(body) });
  },
  updateCodingAgent: (id: string, config: AgentConfigUpdate) =>
    request<void>(`/coding-agents/${encodeURIComponent(id)}/config`, { method: 'PUT', body: JSON.stringify({
      costCapUsd: config.cost_cap_usd,
      timeoutSecs: config.timeout_secs,
      workspaces: config.workspaces,
    }) }),
  deleteCodingAgent: (id: string) =>
    request<void>(`/coding-agents/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  // Coding Agent Tasks
  codingAgentTasks: async (agentId: string, page?: number, pageSize?: number): Promise<ApiResponse<PaginatedResponse<TaskHistoryEntry>>> => {
    const p = page ?? 1;
    const ps = pageSize ?? 20;
    const res = await request<unknown>(
      `/coding-agents/${encodeURIComponent(agentId)}/tasks?page=${p}&page_size=${ps}`
    );
    const raw = res as unknown as { ok: boolean; data?: unknown; items?: unknown[]; total?: number; message?: string };
    if (raw.ok) {
      // Backend may return { ok, data: [] } or { ok, data: { items, total, page, page_size } }
      let items: TaskHistoryEntry[] = [];
      let total = 0;
      if (Array.isArray(raw.data)) {
        items = raw.data as TaskHistoryEntry[];
        total = items.length;
      } else if (raw.data && typeof raw.data === 'object') {
        const d = raw.data as Record<string, unknown>;
        items = (d.items as TaskHistoryEntry[]) ?? [];
        total = (d.total as number) ?? items.length;
      } else if (Array.isArray(raw.items)) {
        items = raw.items as TaskHistoryEntry[];
        total = (raw.total as number) ?? items.length;
      }
      return { ok: true, data: { items, total, page: p, page_size: ps } };
    }
    return { ok: false, message: raw.message || 'Failed to load tasks' };
  },
  codingAgentTask: (agentId: string, taskId: string) =>
    request<TaskDetail>(`/coding-agents/${encodeURIComponent(agentId)}/tasks/${encodeURIComponent(taskId)}`),
  delegateTask: (agentId: string, payload: TaskDelegationPayload) =>
    request<{ task_id: string }>(`/coding-agents/${encodeURIComponent(agentId)}/tasks`, {
      method: 'POST',
      body: JSON.stringify(payload),
    }),

  // Coding Agent Cost
  codingAgentCosts: async (agentId: string): Promise<ApiResponse<AgentCostStats>> => {
    const res = await request<unknown>(`/coding-agents/${encodeURIComponent(agentId)}/costs`);
    const raw = res as unknown as { ok: boolean; data?: Record<string, unknown>; message?: string };
    if (raw.ok && raw.data) {
      const d = raw.data;
      const stats: AgentCostStats = {
        agent_id: (d.agentId as string) ?? (d.agent_id as string) ?? agentId,
        total_input_tokens: (d.totalInputTokens as number) ?? (d.total_input_tokens as number) ?? 0,
        total_output_tokens: (d.totalOutputTokens as number) ?? (d.total_output_tokens as number) ?? 0,
        estimated_total_cost_usd: (d.estimatedTotalCostUsd as number) ?? (d.estimated_total_cost_usd as number) ?? 0,
        task_count: (d.taskCount as number) ?? (d.task_count as number) ?? 0,
        period_start: (d.periodStart as string) ?? (d.period_start as string) ?? new Date().toISOString(),
        period_end: (d.periodEnd as string) ?? (d.period_end as string) ?? new Date().toISOString(),
      };
      return { ok: true, data: stats };
    }
    return { ok: false, message: raw.message || 'Failed to load cost data' };
  },

  // Coding Agent Onboarding
  codingAgentBackends: async (): Promise<ApiResponse<CodingAgentBackend[]>> => {
    const res = await request<unknown>('/coding-agents/backends');
    const raw = res as unknown as { ok: boolean; backends?: CodingAgentBackend[]; data?: CodingAgentBackend[]; message?: string };
    if (raw.ok && raw.backends) {
      return { ok: true, data: raw.backends };
    }
    if (raw.ok && raw.data) {
      return { ok: true, data: raw.data as CodingAgentBackend[] };
    }
    return { ok: false, message: raw.message || 'Failed to load backends' };
  },
  verifyCli: async (backendType: string): Promise<ApiResponse<CliVerificationResult>> => {
    const res = await request<unknown>('/coding-agents/onboarding/check-install', {
      method: 'POST',
      body: JSON.stringify({ agent_type: backendType }),
    });
    const raw = res as unknown as { ok: boolean; installed?: boolean; version?: string | null; path?: string | null; message?: string };
    if (raw.ok && raw.installed !== undefined) {
      return { ok: true, data: { installed: raw.installed, version: raw.version ?? null, path: raw.path ?? null } };
    }
    return { ok: false, message: raw.message || 'Verification failed' };
  },
  listDirectories: async (): Promise<ApiResponse<string[]>> => {
    // The backend doesn't have a directory listing endpoint.
    // Return common workspace paths as suggestions.
    const home = '/Users/' + (window.location.pathname.split('/')[1] || 'user');
    return { ok: true, data: [
      home + '/Developer/projects',
      home + '/Documents',
      '/tmp',
    ]};
  },
};
