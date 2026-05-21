import { useParams, Outlet, useOutletContext } from 'react-router-dom';
import { useApi } from '../../hooks/useApi';
import { api } from '../../api/client';
import SubNavTabs from './components/SubNavTabs';
import type { CodingAgentDetail } from '../../types';

interface AgentDetailContext {
  agent: CodingAgentDetail;
  refetch: () => void;
}

function LoadingSkeleton() {
  return (
    <div className="animate-pulse">
      <div className="h-6 w-48 bg-gray-200 rounded mb-4" />
      <div className="flex border-b border-gray-200 mb-6">
        {[1, 2, 3, 4].map((i) => (
          <div key={i} className="h-4 w-16 bg-gray-200 rounded mx-4 my-2" />
        ))}
      </div>
      <div className="space-y-3">
        <div className="h-4 w-full bg-gray-200 rounded" />
        <div className="h-4 w-3/4 bg-gray-200 rounded" />
        <div className="h-4 w-1/2 bg-gray-200 rounded" />
      </div>
    </div>
  );
}

export default function AgentDetailLayout() {
  const { agentId } = useParams<{ agentId: string }>();
  const { data: agent, loading, error, refetch } = useApi<CodingAgentDetail>(
    () => api.codingAgent(agentId!),
    [agentId]
  );

  const tabs = [
    { to: `/ui/coding-agents/${agentId}`, label: 'Overview' },
    { to: `/ui/coding-agents/${agentId}/tasks`, label: 'Tasks' },
    { to: `/ui/coding-agents/${agentId}/costs`, label: 'Costs' },
    { to: `/ui/coding-agents/${agentId}/new-task`, label: 'New Task' },
    { to: `/ui/coding-agents/${agentId}/settings`, label: 'Settings' },
  ];

  if (loading) {
    return <LoadingSkeleton />;
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center justify-between">
        <p className="text-sm text-red-700">Failed to load agent: {error}</p>
        <button
          onClick={refetch}
          className="px-3 py-1 text-xs font-medium text-red-700 bg-red-100 rounded-lg hover:bg-red-200"
        >
          Retry
        </button>
      </div>
    );
  }

  if (!agent) {
    return (
      <div className="text-center py-16">
        <p className="text-gray-500">Agent not found.</p>
      </div>
    );
  }

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-4">
        {agent.alias || agent.id}
      </h2>
      <SubNavTabs tabs={tabs} />
      <div className="mt-6">
        <Outlet context={{ agent, refetch } satisfies AgentDetailContext} />
      </div>
    </div>
  );
}

/** Hook for child routes to access the agent detail context. */
export function useAgentDetail(): AgentDetailContext {
  return useOutletContext<AgentDetailContext>();
}
