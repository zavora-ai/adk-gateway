import { useState, useCallback } from 'react';
import { useAgentDetail } from './AgentDetailLayout';
import ConnectionBadge from './components/ConnectionBadge';

export default function AgentOverview() {
  const { agent, refetch } = useAgentDetail();
  const [connecting, setConnecting] = useState(false);
  const [actionMessage, setActionMessage] = useState<string | null>(null);

  const handleConnect = useCallback(async () => {
    setConnecting(true);
    setActionMessage(null);
    try {
      const res = await fetch(`/ui/api/coding-agents/${encodeURIComponent(agent.id)}/connect`, {
        method: 'POST',
        credentials: 'same-origin',
      });
      const data = await res.json();
      if (data.ok) {
        setActionMessage('Connected successfully');
        refetch();
      } else {
        setActionMessage(data.message || 'Failed to connect');
      }
    } catch {
      setActionMessage('Network error');
    } finally {
      setConnecting(false);
    }
  }, [agent.id, refetch]);

  const handleDisconnect = useCallback(async () => {
    setConnecting(true);
    setActionMessage(null);
    try {
      const res = await fetch(`/ui/api/coding-agents/${encodeURIComponent(agent.id)}/disconnect`, {
        method: 'POST',
        credentials: 'same-origin',
      });
      const data = await res.json();
      if (data.ok) {
        setActionMessage('Disconnected');
        refetch();
      } else {
        setActionMessage(data.message || 'Failed to disconnect');
      }
    } catch {
      setActionMessage('Network error');
    } finally {
      setConnecting(false);
    }
  }, [agent.id, refetch]);

  return (
    <div className="space-y-6">
      {/* Status Card */}
      <div className="bg-white rounded-xl shadow-sm p-6">
        <h3 className="text-lg font-semibold mb-4">Agent Overview</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
          <div>
            <p className="text-sm text-gray-500">Status</p>
            <div className="mt-1">
              <ConnectionBadge status={agent.connection_status} />
              {agent.status_message && (
                <p className="text-xs text-gray-500 mt-1">{agent.status_message}</p>
              )}
              <div className="mt-2">
                {agent.connection_status === 'connected' ? (
                  <button
                    onClick={handleDisconnect}
                    disabled={connecting}
                    className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100 disabled:opacity-50"
                  >
                    {connecting ? 'Disconnecting…' : 'Disconnect'}
                  </button>
                ) : (
                  <button
                    onClick={handleConnect}
                    disabled={connecting}
                    className="px-3 py-1 text-xs font-medium text-white bg-green-600 rounded-lg hover:bg-green-700 disabled:opacity-50"
                  >
                    {connecting ? 'Connecting…' : '🔌 Connect'}
                  </button>
                )}
              </div>
              {actionMessage && (
                <p className="text-xs text-gray-600 mt-1">{actionMessage}</p>
              )}
            </div>
          </div>
          <div>
            <p className="text-sm text-gray-500">Backend Type</p>
            <p className="text-sm font-medium text-gray-900 mt-1">{agent.display_name}</p>
          </div>
          <div>
            <p className="text-sm text-gray-500">Endpoint</p>
            <p className="text-sm font-mono text-gray-700 mt-1 truncate">{agent.endpoint}</p>
          </div>
          <div>
            <p className="text-sm text-gray-500">Timeout</p>
            <p className="text-sm font-medium text-gray-900 mt-1">{agent.timeout_secs}s</p>
          </div>
          <div>
            <p className="text-sm text-gray-500">Cost Cap</p>
            <p className="text-sm font-medium text-gray-900 mt-1">
              {agent.cost_cap_usd != null ? `$${agent.cost_cap_usd.toFixed(2)}` : 'No limit'}
            </p>
          </div>
          <div>
            <p className="text-sm text-gray-500">Last Task</p>
            <p className="text-sm text-gray-700 mt-1">
              {agent.last_task_at ? new Date(agent.last_task_at).toLocaleString() : 'No tasks yet'}
            </p>
          </div>
        </div>
      </div>

      {/* Workspaces */}
      <div className="bg-white rounded-xl shadow-sm p-6">
        <h3 className="text-lg font-semibold mb-3">Workspaces</h3>
        {agent.workspaces.length > 0 ? (
          <ul className="space-y-1">
            {agent.workspaces.map((ws) => (
              <li key={ws} className="text-sm font-mono text-gray-700 px-3 py-2 bg-gray-50 rounded-lg">
                {ws}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-gray-500">No workspaces configured.</p>
        )}
      </div>

      {/* Capabilities */}
      {agent.capabilities && (
        <div className="bg-white rounded-xl shadow-sm p-6">
          <h3 className="text-lg font-semibold mb-3">Capabilities</h3>
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-4">
            <div className="flex items-center gap-2">
              <span className={`w-2 h-2 rounded-full ${agent.capabilities.file_context ? 'bg-green-500' : 'bg-gray-300'}`} />
              <span className="text-sm text-gray-700">File Context</span>
            </div>
            <div className="flex items-center gap-2">
              <span className={`w-2 h-2 rounded-full ${agent.capabilities.streaming_output ? 'bg-green-500' : 'bg-gray-300'}`} />
              <span className="text-sm text-gray-700">Streaming</span>
            </div>
            <div className="flex items-center gap-2">
              <span className={`w-2 h-2 rounded-full ${agent.capabilities.cost_reporting ? 'bg-green-500' : 'bg-gray-300'}`} />
              <span className="text-sm text-gray-700">Cost Reporting</span>
            </div>
            <div className="flex items-center gap-2">
              <span className={`w-2 h-2 rounded-full ${agent.capabilities.cancellation ? 'bg-green-500' : 'bg-gray-300'}`} />
              <span className="text-sm text-gray-700">Cancellation</span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
