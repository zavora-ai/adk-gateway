import type { ApiResponse } from '../types';

const BASE = '/ui/api';

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
  cronJobs: () => request<{ jobs: import('../types').CronJobInfo[]; total: number }>('/integrations/cron'),
  tools: () => request<{ tools: import('../types').ToolInfo[]; total: number }>('/integrations/tools'),

  // Delegation
  delegationList: () => request<import('../types').DelegationRule[]>('/delegation'),
  delegationAdd: (caller_id: string, target_id: string) =>
    request<void>('/delegation', { method: 'POST', body: JSON.stringify({ caller_id, target_id }) }),
  delegationRemove: (caller_id: string, target_id: string) =>
    request<void>('/delegation', { method: 'DELETE', body: JSON.stringify({ caller_id, target_id }) }),
};
