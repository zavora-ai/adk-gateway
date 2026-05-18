import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useApi } from '../../hooks/useApi';
import { useWebSocket } from '../../hooks/useWebSocket';
import { api } from '../../api/client';
import { useAgentDetail } from './AgentDetailLayout';
import OutcomeBadge from './components/OutcomeBadge';
import type { TaskHistoryEntry, PaginatedResponse, CodingAgentWsEvent } from '../../types';

/** Truncate a string to maxLen characters, appending ellipsis if truncated. */
function truncate(text: string, maxLen: number): string {
  if (text.length <= maxLen) return text;
  return text.slice(0, maxLen) + '…';
}

/** Format duration from milliseconds to a human-readable string. */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const secs = Math.floor(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  const remainSecs = secs % 60;
  if (mins < 60) return remainSecs > 0 ? `${mins}m ${remainSecs}s` : `${mins}m`;
  const hours = Math.floor(mins / 60);
  const remainMins = mins % 60;
  return remainMins > 0 ? `${hours}h ${remainMins}m` : `${hours}h`;
}

/** Format a trigger source to a display string. */
function formatTrigger(trigger: TaskHistoryEntry['trigger']): string {
  switch (trigger.type) {
    case 'userCommand':
      return `User (${trigger.channel})`;
    case 'cronJob':
      return 'Cron Job';
    case 'agentDelegation':
      return 'Agent Delegation';
    case 'controlPanel':
      return 'Control Panel';
    default:
      return 'Unknown';
  }
}

/** Format an ISO timestamp to a locale-friendly display. */
function formatTime(iso: string): string {
  const date = new Date(iso);
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export default function TaskHistoryTable() {
  const { agent } = useAgentDetail();
  const agentId = agent.id;
  const navigate = useNavigate();
  const [page, setPage] = useState(1);
  const [tasks, setTasks] = useState<TaskHistoryEntry[]>([]);
  const [total, setTotal] = useState(0);

  const { data, loading, error, refetch } = useApi<PaginatedResponse<TaskHistoryEntry>>(
    () => api.codingAgentTasks(agentId, page, 20),
    [agentId, page]
  );

  // Sync fetched data into local state
  useEffect(() => {
    if (data) {
      setTasks(data.items);
      setTotal(data.total);
    }
  }, [data]);

  // WebSocket: prepend new tasks for this agent
  const { lastEvent } = useWebSocket();

  useEffect(() => {
    if (!lastEvent) return;
    if (lastEvent.type === 'coding_agent_task') {
      const event = lastEvent as CodingAgentWsEvent & { type: 'coding_agent_task' };
      if (event.agent_id === agentId && page === 1) {
        setTasks((prev) => [event.task, ...prev].slice(0, 20));
        setTotal((prev) => prev + 1);
      }
    }
  }, [lastEvent, agentId, page]);

  const totalPages = Math.max(1, Math.ceil(total / 20));

  const handleRowClick = (taskId: string) => {
    navigate(`/ui/coding-agents/${agentId}/tasks/${taskId}`);
  };

  if (loading && tasks.length === 0) {
    return (
      <div className="bg-white rounded-xl shadow-sm overflow-hidden">
        <div className="animate-pulse p-6 space-y-3">
          {[1, 2, 3, 4, 5].map((i) => (
            <div key={i} className="h-4 bg-gray-200 rounded w-full" />
          ))}
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center justify-between">
        <p className="text-sm text-red-700">Failed to load tasks: {error}</p>
        <button
          onClick={refetch}
          className="px-3 py-1 text-xs font-medium text-red-700 bg-red-100 rounded-lg hover:bg-red-200"
        >
          Retry
        </button>
      </div>
    );
  }

  if (tasks.length === 0) {
    return (
      <div className="bg-white rounded-xl shadow-sm p-8 text-center">
        <p className="text-gray-500">No tasks yet. Delegate a task to get started.</p>
      </div>
    );
  }

  return (
    <div className="bg-white rounded-xl shadow-sm overflow-hidden">
      <div className="overflow-x-auto">
        <table className="w-full">
          <thead>
            <tr className="border-b border-gray-200 bg-gray-50">
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Description
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Trigger
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Started
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Duration
              </th>
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                Outcome
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-100">
            {tasks.map((task) => (
              <tr
                key={task.task_id}
                onClick={() => handleRowClick(task.task_id)}
                className="hover:bg-gray-50 cursor-pointer transition-colors"
              >
                <td className="px-4 py-3 text-sm text-gray-900 whitespace-nowrap max-w-xs">
                  {truncate(task.description, 80)}
                </td>
                <td className="px-4 py-3 text-sm text-gray-600 whitespace-nowrap">
                  {formatTrigger(task.trigger)}
                </td>
                <td className="px-4 py-3 text-sm text-gray-600 whitespace-nowrap">
                  {formatTime(task.started_at)}
                </td>
                <td className="px-4 py-3 text-sm text-gray-600 whitespace-nowrap">
                  {formatDuration(task.duration_ms)}
                </td>
                <td className="px-4 py-3 whitespace-nowrap">
                  <OutcomeBadge outcome={task.outcome} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Pagination Controls */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between px-4 py-3 border-t border-gray-200">
          <p className="text-sm text-gray-600">
            Page {page} of {totalPages} ({total} tasks)
          </p>
          <div className="flex gap-2">
            <button
              onClick={() => setPage((p) => Math.max(1, p - 1))}
              disabled={page <= 1}
              className="px-3 py-1 text-sm font-medium text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Previous
            </button>
            <button
              onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
              disabled={page >= totalPages}
              className="px-3 py-1 text-sm font-medium text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Next
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
