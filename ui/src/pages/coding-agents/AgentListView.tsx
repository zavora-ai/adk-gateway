import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useApi } from '../../hooks/useApi';
import { useWebSocket } from '../../hooks/useWebSocket';
import { api } from '../../api/client';
import ConnectionBadge from './components/ConnectionBadge';
import type { CodingAgentSummary } from '../../types';

function formatTimestamp(iso: string | null): string {
  if (!iso) return 'No tasks yet';
  const date = new Date(iso);
  return date.toLocaleString();
}

interface AgentCardProps {
  agent: CodingAgentSummary;
  onClick: () => void;
}

function AgentCard({ agent, onClick }: AgentCardProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="bg-white rounded-xl shadow-sm p-6 text-left w-full hover:shadow-md transition-shadow cursor-pointer"
    >
      <div className="flex items-start justify-between mb-3">
        <h3 className="text-sm font-semibold text-gray-900 truncate">
          {agent.alias || agent.id}
        </h3>
        <ConnectionBadge status={agent.connection_status} />
      </div>
      <p className="text-xs text-gray-500 mb-2">{agent.display_name}</p>
      <p className="text-xs text-gray-400">
        {formatTimestamp(agent.last_task_at)}
      </p>
    </button>
  );
}

function LoadingSkeleton() {
  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
      {[1, 2, 3].map((i) => (
        <div key={i} className="bg-white rounded-xl shadow-sm p-6 animate-pulse">
          <div className="flex items-start justify-between mb-3">
            <div className="h-4 w-24 bg-gray-200 rounded" />
            <div className="h-5 w-16 bg-gray-200 rounded-full" />
          </div>
          <div className="h-3 w-32 bg-gray-200 rounded mb-2" />
          <div className="h-3 w-20 bg-gray-200 rounded" />
        </div>
      ))}
    </div>
  );
}

export default function AgentListView() {
  const navigate = useNavigate();
  const { data: agents, loading, error, refetch } = useApi<CodingAgentSummary[]>(
    () => api.codingAgents(),
    []
  );
  const { lastEvent } = useWebSocket();
  const [localAgents, setLocalAgents] = useState<CodingAgentSummary[]>([]);

  // Sync local state when API data loads
  useEffect(() => {
    if (agents) {
      setLocalAgents(agents);
    }
  }, [agents]);

  // Handle real-time WebSocket events
  useEffect(() => {
    if (!lastEvent) return;

    if (lastEvent.type === 'coding_agent_status') {
      setLocalAgents(prev =>
        prev.map(a =>
          a.id === lastEvent.agent_id
            ? { ...a, connection_status: lastEvent.status }
            : a
        )
      );
    }

    if (lastEvent.type === 'coding_agent_task') {
      setLocalAgents(prev =>
        prev.map(a =>
          a.id === lastEvent.agent_id
            ? { ...a, last_task_at: lastEvent.task.started_at }
            : a
        )
      );
    }
  }, [lastEvent]);

  if (loading) {
    return (
      <div>
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-2xl font-semibold">Coding Agents</h2>
        </div>
        <LoadingSkeleton />
      </div>
    );
  }

  if (error) {
    return (
      <div>
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-2xl font-semibold">Coding Agents</h2>
        </div>
        <div className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center justify-between">
          <p className="text-sm text-red-700">Failed to load agents: {error}</p>
          <button
            onClick={refetch}
            className="px-3 py-1 text-xs font-medium text-red-700 bg-red-100 rounded-lg hover:bg-red-200"
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  if (localAgents.length === 0) {
    return (
      <div>
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-2xl font-semibold">Coding Agents</h2>
        </div>
        <div className="text-center py-16">
          <p className="text-gray-500 mb-4">No coding agents registered yet.</p>
          <button
            onClick={() => navigate('/ui/coding-agents/new')}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)]"
          >
            Add Agent
          </button>
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <h2 className="text-2xl font-semibold">Coding Agents</h2>
        <button
          onClick={() => navigate('/ui/coding-agents/new')}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)]"
        >
          Add Agent
        </button>
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {localAgents.map((agent) => (
          <AgentCard
            key={agent.id}
            agent={agent}
            onClick={() => navigate(`/ui/coding-agents/${agent.id}`)}
          />
        ))}
      </div>
    </div>
  );
}
