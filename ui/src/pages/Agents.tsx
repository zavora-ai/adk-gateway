import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import ConfirmDialog from '../components/ConfirmDialog';
import AlertBanner from '../components/AlertBanner';
import DelegationPermissions from '../components/DelegationPermissions';
import type { AgentRecord, LogEntry } from '../types';
import { useState, useEffect } from 'react';

const AGENT_TYPES = ['a2a', 'mcp', 'custom'] as const;

interface CreateForm {
  name: string;
  model: string;
  description: string;
  api_key_env: string;
  instruction: string;
  tools: string;
  channel_bindings: string;
  agent_type: string;
  auto_start: boolean;
  allowed_domains: string;
}

const emptyForm: CreateForm = {
  name: '',
  model: '',
  description: '',
  api_key_env: '',
  instruction: '',
  tools: '',
  channel_bindings: '',
  agent_type: 'a2a',
  auto_start: false,
  allowed_domains: '',
};

export default function Agents() {
  const { data, loading, error, refetch } = useApi<AgentRecord[]>(() => api.agents(), []);
  const { lastEvent, isConnected } = useWebSocket();
  const [agents, setAgents] = useState<AgentRecord[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [form, setForm] = useState<CreateForm>(emptyForm);
  const [creating, setCreating] = useState(false);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  // Configure modal
  const [configureAgent, setConfigureAgent] = useState<AgentRecord | null>(null);

  // Log modal
  const [logAgentId, setLogAgentId] = useState<string | null>(null);
  const [logEntries, setLogEntries] = useState<LogEntry[]>([]);
  const [logLoading, setLogLoading] = useState(false);

  // Sync API data into local state
  useEffect(() => {
    if (data) setAgents(data);
  }, [data]);

  // Real-time updates from WebSocket
  useEffect(() => {
    if (lastEvent?.type === 'agent_state') {
      setAgents(prev => prev.map(a =>
        a.id === lastEvent.agent_id ? { ...a, state: lastEvent.state } : a
      ));
    }
  }, [lastEvent]);

  // Polling fallback when WebSocket is disconnected
  useEffect(() => {
    if (isConnected) return;
    const interval = setInterval(() => refetch(), 5000);
    return () => clearInterval(interval);
  }, [isConnected, refetch]);

  const handleStart = async (id: string) => {
    const res = await api.startAgent(id);
    if (res.ok) {
      setAlert({ type: 'success', message: `Agent ${id} started.` });
      refetch();
    } else {
      setAlert({ type: 'error', message: res.message || 'Failed to start.' });
    }
  };

  const handleStop = async (id: string) => {
    const res = await api.stopAgent(id);
    if (res.ok) {
      setAlert({ type: 'success', message: `Agent ${id} stopped.` });
      refetch();
    } else {
      setAlert({ type: 'error', message: res.message || 'Failed to stop.' });
    }
  };

  const handleDelete = async (id: string) => {
    const res = await api.deleteAgent(id);
    if (res.ok) {
      setAlert({ type: 'success', message: `Agent ${id} deleted.` });
      refetch();
    } else {
      setAlert({ type: 'error', message: res.message || 'Failed to delete.' });
    }
    setDeleteTarget(null);
  };

  const handleCreate = async (e: React.FormEvent) => {
    e.preventDefault();
    setCreating(true);
    try {
      const payload = {
        name: form.name,
        model: form.model,
        description: form.description,
        api_key_env: form.api_key_env,
        instruction: form.instruction,
        tools: form.tools.split(',').map((t) => t.trim()).filter(Boolean),
        channel_bindings: form.channel_bindings,
        agent_type: form.agent_type,
        auto_start: form.auto_start,
        allowed_domains: form.allowed_domains.split(',').map((d) => d.trim()).filter(Boolean),
      };
      const res = await api.createAgent(payload);
      if (res.ok) {
        setAlert({ type: 'success', message: 'Agent created.' });
        setForm(emptyForm);
        setShowCreate(false);
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to create.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setCreating(false);
    }
  };

  const openLogs = async (id: string) => {
    setLogAgentId(id);
    setLogLoading(true);
    try {
      const res = await api.agentLogs(id);
      if (res.ok && res.data) {
        setLogEntries(res.data.logs || []);
      }
    } catch {
      setLogEntries([]);
    } finally {
      setLogLoading(false);
    }
  };

  const updateField = (field: keyof CreateForm, value: string | boolean) => {
    setForm((prev) => ({ ...prev, [field]: value }));
  };

  if (loading && agents.length === 0) return <div className="text-gray-400">Loading agents...</div>;
  if (error && agents.length === 0) return <div className="text-red-600">Failed to load agents: {error}</div>;

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <div className="flex items-center gap-3">
          <h2 className="text-2xl font-semibold">Agents</h2>
          <ConnectionIndicator isConnected={isConnected} />
        </div>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)]"
        >
          {showCreate ? 'Cancel' : '+ Create Agent'}
        </button>
      </div>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Create form */}
      {showCreate && (
        <form onSubmit={handleCreate} className="bg-white rounded-xl shadow-sm p-6 mb-6 max-w-2xl">
          <h3 className="text-lg font-semibold mb-4">Create Agent</h3>
          <div className="space-y-3">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Name</label>
              <input
                type="text"
                value={form.name}
                onChange={(e) => updateField('name', e.target.value)}
                required
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Model</label>
              <input
                type="text"
                value={form.model}
                onChange={(e) => updateField('model', e.target.value)}
                placeholder="e.g. gpt-4o"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Description</label>
              <input
                type="text"
                value={form.description}
                onChange={(e) => updateField('description', e.target.value)}
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">API Key Env Variable</label>
              <input
                type="text"
                value={form.api_key_env}
                onChange={(e) => updateField('api_key_env', e.target.value)}
                placeholder="e.g. OPENAI_API_KEY"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Instruction</label>
              <textarea
                value={form.instruction}
                onChange={(e) => updateField('instruction', e.target.value)}
                rows={3}
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Tools (comma-separated)</label>
              <input
                type="text"
                value={form.tools}
                onChange={(e) => updateField('tools', e.target.value)}
                placeholder="web_search, code_exec"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Channel Bindings</label>
              <input
                type="text"
                value={form.channel_bindings}
                onChange={(e) => updateField('channel_bindings', e.target.value)}
                placeholder="e.g. telegram:*, slack:C12345"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Allowed Domains (comma-separated)</label>
              <input
                type="text"
                value={form.allowed_domains}
                onChange={(e) => updateField('allowed_domains', e.target.value)}
                placeholder="e.g. example.com, api.service.io"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div className="flex gap-4">
              <div className="flex-1">
                <label className="block text-sm font-medium text-gray-700 mb-1">Agent Type</label>
                <select
                  value={form.agent_type}
                  onChange={(e) => updateField('agent_type', e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {AGENT_TYPES.map((t) => <option key={t} value={t}>{t}</option>)}
                </select>
              </div>
              <div className="flex items-end pb-1">
                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={form.auto_start}
                    onChange={(e) => updateField('auto_start', e.target.checked)}
                    className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
                  />
                  <span className="text-sm text-gray-600">Auto-start</span>
                </label>
              </div>
            </div>
          </div>
          <button
            type="submit"
            disabled={creating}
            className="mt-4 px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
          >
            {creating ? 'Creating...' : 'Create Agent'}
          </button>
        </form>
      )}

      {/* Agent table */}
      {agents.length === 0 ? (
        <div className="text-center py-12 text-gray-400">No agents configured</div>
      ) : (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Name</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">State</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Port</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Model</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {agents.map((agent) => (
                <AgentRow
                  key={agent.id}
                  agent={agent}
                  expanded={expandedId === agent.id}
                  onToggle={() => setExpandedId(expandedId === agent.id ? null : agent.id)}
                  onStart={() => handleStart(agent.id)}
                  onStop={() => handleStop(agent.id)}
                  onDelete={() => setDeleteTarget(agent.id)}
                  onLogs={() => openLogs(agent.id)}
                  onConfigure={() => setConfigureAgent(agent)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Delete confirmation */}
      {deleteTarget && (
        <ConfirmDialog
          title="Delete Agent"
          message={`Are you sure you want to delete agent "${deleteTarget}"? This action cannot be undone.`}
          confirmLabel="Delete"
          destructive
          onConfirm={() => handleDelete(deleteTarget)}
          onCancel={() => setDeleteTarget(null)}
        />
      )}

      {/* Log modal */}
      {logAgentId && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white rounded-xl shadow-xl max-w-3xl w-full max-h-[80vh] flex flex-col">
            <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200">
              <h3 className="text-lg font-semibold">Logs: {logAgentId}</h3>
              <button
                onClick={() => setLogAgentId(null)}
                className="text-gray-400 hover:text-gray-600 text-xl"
              >
                ×
              </button>
            </div>
            <div className="flex-1 overflow-auto p-4">
              {logLoading ? (
                <div className="text-gray-400 text-center py-8">Loading logs...</div>
              ) : logEntries.length === 0 ? (
                <div className="text-gray-400 text-center py-8">No logs available</div>
              ) : (
                <table className="w-full">
                  <thead>
                    <tr className="bg-gray-50">
                      <th className="text-left px-3 py-2 text-xs uppercase text-gray-500">Time</th>
                      <th className="text-left px-3 py-2 text-xs uppercase text-gray-500">Level</th>
                      <th className="text-left px-3 py-2 text-xs uppercase text-gray-500">Message</th>
                    </tr>
                  </thead>
                  <tbody>
                    {logEntries.map((log, i) => (
                      <tr key={i} className="border-t border-gray-100">
                        <td className="px-3 py-2 text-xs font-mono text-gray-500">{log.timestamp}</td>
                        <td className="px-3 py-2"><StatusBadge status={log.level} /></td>
                        <td className="px-3 py-2 text-sm break-all">{log.message}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Configure modal */}
      {configureAgent && (
        <ConfigureModal
          agent={configureAgent}
          onClose={() => setConfigureAgent(null)}
          onSuccess={(updatedAgent) => {
            setAgents(prev => prev.map(a => a.id === updatedAgent.id ? updatedAgent : a));
            setAlert({ type: 'success', message: `Agent '${updatedAgent.name}' configuration updated.` });
            setConfigureAgent(null);
          }}
          onError={(message) => {
            setAlert({ type: 'error', message });
          }}
        />
      )}

      {/* Delegation Permissions Section */}
      <DelegationPermissions agents={agents} />
    </div>
  );
}

function AgentRow({
  agent,
  expanded,
  onToggle,
  onStart,
  onStop,
  onDelete,
  onLogs,
  onConfigure,
}: {
  agent: AgentRecord;
  expanded: boolean;
  onToggle: () => void;
  onStart: () => void;
  onStop: () => void;
  onDelete: () => void;
  onLogs: () => void;
  onConfigure: () => void;
}) {
  const state = agent.state.toLowerCase();
  const isSystem = agent.id === 'system';
  const isRunning = state === 'running';
  const isTransitioning = state === 'starting' || state === 'stopping';
  const canStart = state === 'created' || state === 'stopped' || state === 'error';

  return (
    <>
      <tr
        className="border-t border-gray-100 hover:bg-gray-50 cursor-pointer"
        onClick={onToggle}
      >
        <td className="px-4 py-3 text-sm font-medium">
          <span className="flex items-center gap-2">
            {agent.name}
            {isSystem && (
              <span className="inline-block px-1.5 py-0.5 rounded text-[10px] font-semibold bg-indigo-100 text-indigo-700 uppercase tracking-wide">
                System
              </span>
            )}
          </span>
        </td>
        <td className="px-4 py-3 text-sm font-mono text-gray-500">{agent.id}</td>
        <td className="px-4 py-3"><StatusBadge status={agent.state} /></td>
        <td className="px-4 py-3 text-sm text-gray-500">{agent.port ?? '—'}</td>
        <td className="px-4 py-3 text-sm text-gray-600">{agent.model}</td>
        <td className="px-4 py-3">
          <div className="flex gap-2" onClick={(e) => e.stopPropagation()}>
            {!isSystem && (
              <>
                {isTransitioning ? (
                  <span className="px-3 py-1 text-xs font-medium text-gray-500 bg-gray-100 rounded-lg inline-flex items-center gap-1">
                    <svg className="animate-spin h-3 w-3" viewBox="0 0 24 24" fill="none">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
                    </svg>
                    {state === 'starting' ? 'Starting…' : 'Stopping…'}
                  </span>
                ) : isRunning ? (
                  <button
                    onClick={onStop}
                    className="px-3 py-1 text-xs font-medium text-yellow-700 bg-yellow-50 rounded-lg hover:bg-yellow-100"
                  >
                    Stop
                  </button>
                ) : canStart ? (
                  <button
                    onClick={onStart}
                    className="px-3 py-1 text-xs font-medium text-green-700 bg-green-50 rounded-lg hover:bg-green-100"
                  >
                    Start
                  </button>
                ) : null}
              </>
            )}
            <button
              onClick={onConfigure}
              className="px-3 py-1 text-xs font-medium text-purple-700 bg-purple-50 rounded-lg hover:bg-purple-100"
            >
              Configure
            </button>
            <button
              onClick={onLogs}
              className="px-3 py-1 text-xs font-medium text-blue-700 bg-blue-50 rounded-lg hover:bg-blue-100"
            >
              Logs
            </button>
            {!isSystem && (
              <button
                onClick={onDelete}
                className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
              >
                Delete
              </button>
            )}
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="bg-gray-50">
          <td colSpan={6} className="px-6 py-4 text-sm text-gray-600">
            <div className="grid grid-cols-2 gap-x-8 gap-y-2">
              <div><strong>Type:</strong> {agent.agent_type}</div>
              <div><strong>Auto-start:</strong> {agent.auto_start ? 'Yes' : 'No'}</div>
              <div><strong>Created:</strong> {agent.created_at}</div>
              <div><strong>API Key Env:</strong> {agent.api_key_env || '—'}</div>
              <div className="col-span-2"><strong>Tools:</strong> {agent.tools.length > 0 ? agent.tools.join(', ') : '—'}</div>
              <div className="col-span-2">
                <strong>Channel Bindings:</strong>{' '}
                {agent.channel_bindings.length > 0
                  ? agent.channel_bindings.map((b) => `${b.channel_type}${b.account_id ? ':' + b.account_id : ''}`).join(', ')
                  : '—'}
              </div>
              {agent.instruction && (
                <div className="col-span-2">
                  <strong>Instruction:</strong>
                  <pre className="mt-1 bg-gray-100 rounded-lg p-3 text-xs font-mono whitespace-pre-wrap">{agent.instruction}</pre>
                </div>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

// ── Connection Status Indicator ────────────────────────────────────

function ConnectionIndicator({ isConnected }: { isConnected: boolean }) {
  return (
    <div className="flex items-center gap-1.5" title={isConnected ? 'Real-time updates active' : 'Reconnecting...'}>
      <span
        className={`inline-block w-2 h-2 rounded-full ${
          isConnected ? 'bg-green-500' : 'bg-yellow-500 animate-pulse'
        }`}
      />
      <span className="text-xs text-gray-500">
        {isConnected ? 'Live' : 'Reconnecting'}
      </span>
    </div>
  );
}

// ── Configure Modal ────────────────────────────────────────────────

interface ConfigureForm {
  model: string;
  instruction: string;
  tools: string;
  channel_bindings: string;
  auto_start: boolean;
  allowed_domains: string;
}

function ConfigureModal({
  agent,
  onClose,
  onSuccess,
  onError,
}: {
  agent: AgentRecord;
  onClose: () => void;
  onSuccess: (updatedAgent: AgentRecord) => void;
  onError: (message: string) => void;
}) {
  const [form, setForm] = useState<ConfigureForm>({
    model: agent.model,
    instruction: agent.instruction,
    tools: agent.tools.join(', '),
    channel_bindings: agent.channel_bindings
      .map((b) => `${b.channel_type}${b.account_id ? ':' + b.account_id : ''}`)
      .join(', '),
    auto_start: agent.auto_start,
    allowed_domains: '',
  });
  const [submitting, setSubmitting] = useState(false);

  const updateField = (field: keyof ConfigureForm, value: string | boolean) => {
    setForm((prev) => ({ ...prev, [field]: value }));
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    try {
      const payload = {
        model: form.model,
        instruction: form.instruction,
        tools: form.tools.split(',').map((t) => t.trim()).filter(Boolean),
        channel_bindings: form.channel_bindings,
        auto_start: form.auto_start,
        allowed_domains: form.allowed_domains.split(',').map((d) => d.trim()).filter(Boolean),
      };
      const res = await api.post<void>(`/agents/${encodeURIComponent(agent.id)}/configure`, payload);
      if (res.ok) {
        const updatedAgent: AgentRecord = {
          ...agent,
          model: form.model,
          instruction: form.instruction,
          tools: payload.tools,
          auto_start: form.auto_start,
          updated_at: new Date().toISOString(),
        };
        onSuccess(updatedAgent);
      } else {
        onError(res.message || 'Failed to update agent configuration.');
      }
    } catch {
      onError('Network error while updating configuration.');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white rounded-xl shadow-xl max-w-lg w-full max-h-[90vh] overflow-auto">
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200">
          <h3 className="text-lg font-semibold">Configure: {agent.name}</h3>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 text-xl"
          >
            ×
          </button>
        </div>
        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Model</label>
            <input
              type="text"
              value={form.model}
              onChange={(e) => updateField('model', e.target.value)}
              placeholder="e.g. gpt-4o"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Instruction</label>
            <textarea
              value={form.instruction}
              onChange={(e) => updateField('instruction', e.target.value)}
              rows={4}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Tools (comma-separated)</label>
            <input
              type="text"
              value={form.tools}
              onChange={(e) => updateField('tools', e.target.value)}
              placeholder="web_search, code_exec"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Channel Bindings</label>
            <input
              type="text"
              value={form.channel_bindings}
              onChange={(e) => updateField('channel_bindings', e.target.value)}
              placeholder="e.g. telegram:*, slack:C12345"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Allowed Domains (comma-separated)</label>
            <input
              type="text"
              value={form.allowed_domains}
              onChange={(e) => updateField('allowed_domains', e.target.value)}
              placeholder="e.g. example.com, api.service.io"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={form.auto_start}
              onChange={(e) => updateField('auto_start', e.target.checked)}
              className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
            />
            <label className="text-sm text-gray-600">Auto-start on gateway boot</label>
          </div>
          <div className="flex justify-end gap-3 pt-2">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={submitting}
              className="px-6 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
            >
              {submitting ? 'Saving...' : 'Save Configuration'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
