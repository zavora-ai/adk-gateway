import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import type { McpServerInfo, CronJobInfo, ToolInfo, AcpAgentInfo, AcpAgentForm } from '../types';
import { useState } from 'react';
import AlertBanner from '../components/AlertBanner';
import ConfirmDialog from '../components/ConfirmDialog';

export default function Integrations() {
  const { data: mcpServers, refetch: refetchMcp } = useApi<McpServerInfo[]>(() => api.mcpServers(), []);
  const { data: cronData, refetch: refetchCron } = useApi<{ jobs: CronJobInfo[]; total: number }>(() => api.cronJobs(), []);
  const { data: toolsData } = useApi<{ tools: ToolInfo[]; total: number }>(() => api.tools(), []);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  // MCP Add form state
  const [mcpForm, setMcpForm] = useState({
    server_id: '',
    transport: 'stdio' as 'stdio' | 'http',
    command: '',
    args: '',
    url: '',
    env: '',
    disabled: false,
  });
  const [jsonMode, setJsonMode] = useState(false);
  const [jsonInput, setJsonInput] = useState('');

  const cronJobs = cronData?.jobs ?? [];
  const tools = toolsData?.tools ?? [];

  const handleAddMcp = async (e: React.FormEvent) => {
    e.preventDefault();

    if (jsonMode) {
      // JSON paste mode
      if (!jsonInput.trim()) {
        setAlert({ type: 'error', message: 'Paste a JSON config block.' });
        return;
      }
      let parsed: Record<string, unknown>;
      try {
        parsed = JSON.parse(jsonInput.trim());
      } catch {
        setAlert({ type: 'error', message: 'Invalid JSON. Expected: {"server-name": {"command": "...", "args": [...]}}' });
        return;
      }

      // Support both {"mcpServers": {...}} wrapper and direct {"name": {...}} format
      const servers = (parsed.mcpServers as Record<string, unknown>) ?? parsed;
      let addedCount = 0;

      for (const [serverId, serverConfig] of Object.entries(servers)) {
        if (typeof serverConfig !== 'object' || serverConfig === null) continue;
        const cfg = serverConfig as Record<string, unknown>;

        const payload: Record<string, unknown> = {
          server_id: serverId,
          disabled: cfg.disabled ?? false,
        };

        if (cfg.command) {
          payload.transport = 'stdio';
          payload.command = cfg.command;
          payload.args = cfg.args ?? [];
          payload.env = cfg.env ?? {};
        } else if (cfg.url) {
          payload.transport = 'http';
          payload.url = cfg.url;
        } else {
          setAlert({ type: 'error', message: `Server '${serverId}' needs 'command' or 'url'.` });
          return;
        }

        try {
          const res = await api.addMcpServer(payload);
          if (res.ok) {
            addedCount++;
          } else {
            setAlert({ type: 'error', message: res.message || `Failed to add '${serverId}'.` });
            return;
          }
        } catch {
          setAlert({ type: 'error', message: 'Network error.' });
          return;
        }
      }

      if (addedCount > 0) {
        setAlert({ type: 'success', message: `Added ${addedCount} MCP server${addedCount > 1 ? 's' : ''}.` });
        setJsonInput('');
        refetchMcp();
      }
      return;
    }

    // Form mode
    if (!mcpForm.server_id.trim()) {
      setAlert({ type: 'error', message: 'Server ID is required.' });
      return;
    }

    const payload: Record<string, unknown> = {
      server_id: mcpForm.server_id.trim(),
      transport: mcpForm.transport,
      disabled: mcpForm.disabled,
    };

    if (mcpForm.transport === 'stdio') {
      if (!mcpForm.command.trim()) {
        setAlert({ type: 'error', message: 'Command is required for stdio transport.' });
        return;
      }
      payload.command = mcpForm.command.trim();
      payload.args = mcpForm.args.trim() ? mcpForm.args.trim().split(/\s+/) : [];
      // Parse env vars from textarea (KEY=VALUE per line)
      const envMap: Record<string, string> = {};
      if (mcpForm.env.trim()) {
        for (const line of mcpForm.env.trim().split('\n')) {
          const idx = line.indexOf('=');
          if (idx > 0) {
            envMap[line.slice(0, idx).trim()] = line.slice(idx + 1).trim();
          }
        }
      }
      payload.env = envMap;
    } else {
      if (!mcpForm.url.trim()) {
        setAlert({ type: 'error', message: 'URL is required for HTTP transport.' });
        return;
      }
      payload.url = mcpForm.url.trim();
    }

    try {
      const res = await api.addMcpServer(payload);
      if (res.ok) {
        setAlert({ type: 'success', message: `MCP server '${mcpForm.server_id}' added.` });
        setMcpForm({ server_id: '', transport: 'stdio', command: '', args: '', url: '', env: '', disabled: false });
        refetchMcp();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to add MCP server.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  const handleToggleMcp = async (id: string) => {
    try {
      const res = await api.toggleMcpServer(id);
      if (res.ok) {
        refetchMcp();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to toggle.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  const handleRemoveMcp = async (id: string) => {
    try {
      const res = await api.removeMcpServer(id);
      if (res.ok) {
        setAlert({ type: 'success', message: `MCP server '${id}' removed.` });
        refetchMcp();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to remove.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
    setConfirmRemove(null);
  };

  const cancelCronJob = async (id: string) => {
    try {
      const res = await api.post(`/integrations/cron/${encodeURIComponent(id)}/cancel`);
      if (res.ok) {
        setAlert({ type: 'success', message: `Cron job ${id} cancelled.` });
        refetchCron();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to cancel.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
  };

  // Group tools by source
  const toolsBySource: Record<string, ToolInfo[]> = {};
  if (tools) {
    for (const tool of tools) {
      const source = tool.source || 'unknown';
      if (!toolsBySource[source]) toolsBySource[source] = [];
      toolsBySource[source].push(tool);
    }
  }

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Integrations</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {confirmRemove && (
        <ConfirmDialog
          title="Remove MCP Server"
          message={`Are you sure you want to remove MCP server '${confirmRemove}'?`}
          onConfirm={() => handleRemoveMcp(confirmRemove)}
          onCancel={() => setConfirmRemove(null)}
        />
      )}

      {/* Add MCP Server Form */}
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-lg font-semibold">Add MCP Server</h3>
        <button
          type="button"
          onClick={() => setJsonMode(!jsonMode)}
          className="px-3 py-1 text-xs font-medium text-gray-600 bg-gray-100 rounded-lg hover:bg-gray-200 transition-colors"
        >
          {jsonMode ? '← Form Mode' : 'Paste JSON'}
        </button>
      </div>
      <form onSubmit={handleAddMcp} className="bg-white rounded-xl shadow-sm p-5 mb-6 space-y-4">
        {jsonMode ? (
          <>
            <p className="text-sm text-gray-500">
              Paste a JSON config block. Supports both formats:
            </p>
            <pre className="text-xs text-gray-400 bg-gray-50 rounded-lg p-3 font-mono">
{`{"server-name": {"command": "uvx", "args": ["pkg@latest"], "env": {"KEY": "val"}}}

// or with wrapper:
{"mcpServers": {"server-name": {"command": "uvx", "args": ["pkg@latest"]}}}`}
            </pre>
            <textarea
              value={jsonInput}
              onChange={(e) => setJsonInput(e.target.value)}
              placeholder='{"my-server": {"command": "uvx", "args": ["package@latest"]}}'
              rows={8}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
            <div className="flex justify-end">
              <button
                type="submit"
                className="px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-700 transition-colors"
              >
                Add from JSON
              </button>
            </div>
          </>
        ) : (
          <>
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Server ID</label>
            <input
              type="text"
              value={mcpForm.server_id}
              onChange={(e) => setMcpForm({ ...mcpForm, server_id: e.target.value })}
              placeholder="my-mcp-server"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Transport</label>
            <select
              value={mcpForm.transport}
              onChange={(e) => setMcpForm({ ...mcpForm, transport: e.target.value as 'stdio' | 'http' })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            >
              <option value="stdio">stdio</option>
              <option value="http">http</option>
            </select>
          </div>
        </div>

        {mcpForm.transport === 'stdio' ? (
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Command</label>
              <input
                type="text"
                value={mcpForm.command}
                onChange={(e) => setMcpForm({ ...mcpForm, command: e.target.value })}
                placeholder="uvx"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Args (space-separated)</label>
              <input
                type="text"
                value={mcpForm.args}
                onChange={(e) => setMcpForm({ ...mcpForm, args: e.target.value })}
                placeholder="package@latest --flag"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
              />
            </div>
          </div>
        ) : (
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">URL</label>
            <input
              type="text"
              value={mcpForm.url}
              onChange={(e) => setMcpForm({ ...mcpForm, url: e.target.value })}
              placeholder="https://api.example.com/mcp"
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
            />
          </div>
        )}

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Environment Variables (one KEY=VALUE per line)</label>
          <textarea
            value={mcpForm.env}
            onChange={(e) => setMcpForm({ ...mcpForm, env: e.target.value })}
            placeholder={"API_KEY=sk-xxx\nDEBUG=true"}
            rows={3}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
          />
        </div>

        <div className="flex items-center justify-between">
          <label className="flex items-center gap-2 text-sm text-gray-700">
            <input
              type="checkbox"
              checked={!mcpForm.disabled}
              onChange={(e) => setMcpForm({ ...mcpForm, disabled: !e.target.checked })}
              className="rounded border-gray-300"
            />
            Enabled
          </label>
          <button
            type="submit"
            className="px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-700 transition-colors"
          >
            Add Server
          </button>
        </div>
          </>
        )}
      </form>

      {/* MCP Servers Table */}
      <h3 className="text-lg font-semibold mb-3">MCP Servers</h3>
      {mcpServers && mcpServers.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Server ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Transport</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Discovered Tools</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {mcpServers.map((srv) => (
                <tr key={srv.server_id} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-mono">{srv.server_id}</td>
                  <td className="px-4 py-3 text-sm text-gray-600">{srv.transport ?? '—'}</td>
                  <td className="px-4 py-3"><StatusBadge status={srv.status} /></td>
                  <td className="px-4 py-3 text-sm text-gray-600">
                    {(srv.tools ?? srv.discovered_tools ?? []).length > 0 ? (srv.tools ?? srv.discovered_tools ?? []).join(', ') : <span className="text-gray-400">None</span>}
                  </td>
                  <td className="px-4 py-3 flex gap-2">
                    <button
                      onClick={() => handleToggleMcp(srv.server_id)}
                      className={`px-3 py-1 text-xs font-medium rounded-lg ${
                        srv.enabled !== false
                          ? 'text-yellow-700 bg-yellow-50 hover:bg-yellow-100'
                          : 'text-green-700 bg-green-50 hover:bg-green-100'
                      }`}
                    >
                      {srv.enabled !== false ? 'Disable' : 'Enable'}
                    </button>
                    <button
                      onClick={() => setConfirmRemove(srv.server_id)}
                      className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm mb-6">
          No MCP servers configured
        </div>
      )}

      {/* Cron Jobs */}
      <h3 className="text-lg font-semibold mb-3">Cron Jobs</h3>
      {cronJobs && cronJobs.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Job ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Schedule</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {cronJobs.map((job) => (
                <tr key={job.id} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-mono">{job.id}</td>
                  <td className="px-4 py-3 text-sm font-mono">{job.schedule}</td>
                  <td className="px-4 py-3"><StatusBadge status={job.status} /></td>
                  <td className="px-4 py-3">
                    <button
                      onClick={() => cancelCronJob(job.id)}
                      className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                    >
                      Cancel
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm mb-6">
          No cron jobs scheduled
        </div>
      )}

      {/* Tools */}
      <h3 className="text-lg font-semibold mb-3">Tools</h3>
      {tools && tools.length > 0 ? (
        <div className="space-y-4 mb-6">
          {Object.entries(toolsBySource).map(([source, sourceTools]) => (
            <div key={source} className="bg-white rounded-xl shadow-sm p-4">
              <h4 className="text-sm font-semibold text-gray-500 mb-2 uppercase">{source}</h4>
              <div className="flex flex-wrap gap-2">
                {sourceTools.map((tool) => (
                  <span
                    key={tool.name}
                    className="px-3 py-1 bg-gray-100 text-gray-700 rounded-lg text-sm"
                    title={tool.description}
                  >
                    {tool.name}
                  </span>
                ))}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm mb-6">
          No tools registered
        </div>
      )}

      {/* Plugins */}
      <h3 className="text-lg font-semibold mb-3">Plugins</h3>
      <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm">
        No plugins installed
      </div>

      {/* ACP Agents (Task 17) */}
      <AcpAgentsSection />
    </div>
  );
}

// ── ACP Agents Section (Task 17) ──────────────────────────────────

function AcpAgentsSection() {
  const { data: acpFeature } = useApi<{ enabled: boolean }>(() => api.getAcpFeatureEnabled(), []);
  const { data: acpAgents, refetch } = useApi<AcpAgentInfo[]>(() => api.getAcpAgents(), []);
  const [showForm, setShowForm] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [form, setForm] = useState<AcpAgentForm>({
    name: '',
    command: '',
    working_directory: '',
    auto_approve: false,
    timeout_secs: 300,
  });

  // Only show if ACP feature is enabled
  if (!acpFeature?.enabled) return null;

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.name || !form.command) return;
    await api.addAcpAgent(form);
    setForm({ name: '', command: '', working_directory: '', auto_approve: false, timeout_secs: 300 });
    setShowForm(false);
    refetch();
  };

  const handleRemove = async (name: string) => {
    await api.removeAcpAgent(name);
    setConfirmRemove(null);
    refetch();
  };

  return (
    <div className="mt-8">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-lg font-semibold">ACP Agents</h3>
        <button
          onClick={() => setShowForm(!showForm)}
          className="px-3 py-1.5 text-xs font-medium text-[var(--color-accent)] bg-blue-50 rounded-lg hover:bg-blue-100"
        >
          {showForm ? 'Cancel' : '+ Add ACP Agent'}
        </button>
      </div>

      {confirmRemove && (
        <ConfirmDialog
          title="Remove ACP Agent"
          message={`Are you sure you want to remove ACP agent '${confirmRemove}'?`}
          onConfirm={() => handleRemove(confirmRemove)}
          onCancel={() => setConfirmRemove(null)}
          destructive
        />
      )}

      {/* Add form */}
      {showForm && (
        <form onSubmit={handleAdd} className="bg-white rounded-xl shadow-sm p-5 mb-4 space-y-3">
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Name</label>
              <input
                type="text"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="e.g. claude-code"
                required
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Command</label>
              <input
                type="text"
                value={form.command}
                onChange={(e) => setForm({ ...form, command: e.target.value })}
                placeholder="e.g. claude-code"
                required
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Working Directory</label>
              <input
                type="text"
                value={form.working_directory}
                onChange={(e) => setForm({ ...form, working_directory: e.target.value })}
                placeholder="/path/to/project"
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-1">Timeout (seconds)</label>
              <input
                type="number"
                min="30"
                max="3600"
                value={form.timeout_secs}
                onChange={(e) => setForm({ ...form, timeout_secs: parseInt(e.target.value, 10) || 300 })}
                className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
            </div>
          </div>
          <div className="flex items-center justify-between">
            <label className="flex items-center gap-2 text-sm text-gray-700">
              <input
                type="checkbox"
                checked={form.auto_approve}
                onChange={(e) => setForm({ ...form, auto_approve: e.target.checked })}
                className="rounded border-gray-300"
              />
              Auto-approve tool calls
            </label>
            <button
              type="submit"
              className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)]"
            >
              Add Agent
            </button>
          </div>
        </form>
      )}

      {/* ACP Agents Table */}
      {acpAgents && acpAgents.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Name</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Command</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Timeout</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {acpAgents.map((agent) => (
                <tr key={agent.name} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-medium">{agent.name}</td>
                  <td className="px-4 py-3 text-sm font-mono text-gray-600">{agent.command}</td>
                  <td className="px-4 py-3"><StatusBadge status={agent.status} /></td>
                  <td className="px-4 py-3 text-sm text-gray-500">{agent.timeout_secs}s</td>
                  <td className="px-4 py-3">
                    <button
                      onClick={() => setConfirmRemove(agent.name)}
                      className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm">
          No ACP agents configured
        </div>
      )}
    </div>
  );
}
