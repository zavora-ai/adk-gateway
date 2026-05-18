import { useState, useEffect } from 'react';
import { useApi } from '../../hooks/useApi';
import { useWebSocket } from '../../hooks/useWebSocket';
import { api } from '../../api/client';
import { useAgentDetail } from './AgentDetailLayout';
import type { AgentCostStats, WsEvent } from '../../types';

function LoadingSkeleton() {
  return (
    <div className="bg-white rounded-xl shadow-sm p-6 animate-pulse">
      <div className="h-5 w-40 bg-gray-200 rounded mb-6" />
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {[1, 2, 3, 4, 5].map((i) => (
          <div key={i} className="space-y-2">
            <div className="h-3 w-24 bg-gray-200 rounded" />
            <div className="h-6 w-32 bg-gray-200 rounded" />
          </div>
        ))}
      </div>
    </div>
  );
}

export default function CostStatisticsPanel() {
  const { agent } = useAgentDetail();
  const agentId = agent.id;

  const { data: costData, loading, error, refetch } = useApi<AgentCostStats>(
    () => api.codingAgentCosts(agentId),
    [agentId]
  );

  const { lastEvent } = useWebSocket();
  const [costWarning, setCostWarning] = useState(false);

  useEffect(() => {
    if (!lastEvent) return;
    const event = lastEvent as WsEvent;
    if (
      event.type === 'coding_agent_cost_warning' &&
      event.agent_id === agentId
    ) {
      setCostWarning(true);
    }
  }, [lastEvent, agentId]);

  if (loading) {
    return <LoadingSkeleton />;
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center justify-between">
        <p className="text-sm text-red-700">Failed to load cost data: {error}</p>
        <button
          onClick={refetch}
          className="px-3 py-1 text-xs font-medium text-red-700 bg-red-100 rounded-lg hover:bg-red-200"
        >
          Retry
        </button>
      </div>
    );
  }

  if (!costData) {
    return (
      <div className="bg-white rounded-xl shadow-sm p-6 text-center py-16">
        <p className="text-gray-500">No cost data available yet.</p>
        <p className="text-sm text-gray-400 mt-1">
          Cost statistics will appear here once the agent completes tasks.
        </p>
      </div>
    );
  }

  const averageCostPerTask =
    costData.task_count > 0
      ? costData.estimated_total_cost_usd / costData.task_count
      : 0;

  const periodStart = new Date(costData.period_start).toLocaleDateString();
  const periodEnd = new Date(costData.period_end).toLocaleDateString();

  const cardClasses = costWarning
    ? 'bg-white rounded-xl shadow-sm p-6 border-2 border-amber-400 bg-amber-50'
    : 'bg-white rounded-xl shadow-sm p-6';

  return (
    <div className={cardClasses}>
      <div className="flex items-center justify-between mb-6">
        <h3 className="text-lg font-semibold">Cost Statistics</h3>
        {costWarning && (
          <span className="px-2 py-0.5 rounded-full text-xs font-medium bg-amber-100 text-amber-700">
            Cost threshold exceeded
          </span>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
        <div>
          <p className="text-sm text-gray-500">Total Input Tokens</p>
          <p className="text-xl font-semibold mt-1">
            {costData.total_input_tokens.toLocaleString()}
          </p>
        </div>

        <div>
          <p className="text-sm text-gray-500">Total Output Tokens</p>
          <p className="text-xl font-semibold mt-1">
            {costData.total_output_tokens.toLocaleString()}
          </p>
        </div>

        <div>
          <p className="text-sm text-gray-500">Estimated Total Cost</p>
          <p className="text-xl font-semibold mt-1">
            ${costData.estimated_total_cost_usd.toFixed(4)}
          </p>
        </div>

        <div>
          <p className="text-sm text-gray-500">Average Cost Per Task</p>
          <p className="text-xl font-semibold mt-1">
            {costData.task_count > 0
              ? `$${averageCostPerTask.toFixed(4)}`
              : '$0.00'}
          </p>
        </div>

        <div>
          <p className="text-sm text-gray-500">Billing Period</p>
          <p className="text-xl font-semibold mt-1">
            {periodStart} – {periodEnd}
          </p>
        </div>

        <div>
          <p className="text-sm text-gray-500">Total Tasks</p>
          <p className="text-xl font-semibold mt-1">
            {costData.task_count.toLocaleString()}
          </p>
        </div>
      </div>
    </div>
  );
}
