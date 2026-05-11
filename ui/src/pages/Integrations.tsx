import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import type { McpServerInfo, CronJobInfo, ToolInfo } from '../types';
import { useState } from 'react';
import AlertBanner from '../components/AlertBanner';

export default function Integrations() {
  const { data: mcpServers } = useApi<McpServerInfo[]>(() => api.mcpServers(), []);
  const { data: cronData, refetch: refetchCron } = useApi<{ jobs: CronJobInfo[]; total: number }>(() => api.cronJobs(), []);
  const { data: toolsData } = useApi<{ tools: ToolInfo[]; total: number }>(() => api.tools(), []);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  const cronJobs = cronData?.jobs ?? [];
  const tools = toolsData?.tools ?? [];

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

      {/* MCP Servers */}
      <h3 className="text-lg font-semibold mb-3">MCP Servers</h3>
      {mcpServers && mcpServers.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Server ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Discovered Tools</th>
              </tr>
            </thead>
            <tbody>
              {mcpServers.map((srv) => (
                <tr key={srv.server_id} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-mono">{srv.server_id}</td>
                  <td className="px-4 py-3"><StatusBadge status={srv.status} /></td>
                  <td className="px-4 py-3 text-sm text-gray-600">
                    {(srv.tools ?? srv.discovered_tools ?? []).length > 0 ? (srv.tools ?? srv.discovered_tools ?? []).join(', ') : <span className="text-gray-400">None</span>}
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
    </div>
  );
}
