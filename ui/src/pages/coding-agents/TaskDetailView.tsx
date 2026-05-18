import { useParams, useNavigate } from 'react-router-dom';
import { useApi } from '../../hooks/useApi';
import { api } from '../../api/client';
import OutcomeBadge from './components/OutcomeBadge';
import type { TaskDetail, TaskTrigger, TaskError } from '../../types';

/** Format a TaskTrigger into a human-readable string. */
function formatTrigger(trigger: TaskTrigger): string {
  switch (trigger.type) {
    case 'userCommand':
      return `User command (${trigger.channel})`;
    case 'cronJob':
      return `Cron job (${trigger.job_id})`;
    case 'agentDelegation':
      return `Agent delegation (${trigger.source_agent_id})`;
    case 'controlPanel':
      return `Control Panel (${trigger.user_id})`;
  }
}

/** Format milliseconds into a human-readable duration. */
function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  if (minutes < 60) return `${minutes}m ${remainingSeconds}s`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return `${hours}h ${remainingMinutes}m`;
}

/** Format an ISO date string into a readable local date/time. */
function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** Get a human-readable error message from a TaskError. */
function getErrorMessage(error: TaskError): string {
  switch (error.category) {
    case 'timeout':
      return `Task timed out after ${error.elapsed_secs}s (limit: ${error.limit_secs}s)`;
    case 'costCap':
      return `Cost cap exceeded: spent $${error.spent_usd.toFixed(4)} (cap: $${error.cap_usd.toFixed(4)})`;
    case 'rateLimit':
      return error.retry_after_secs
        ? `Rate limited. Retry after ${error.retry_after_secs}s`
        : 'Rate limited';
    case 'executionError':
      return error.message;
    case 'agentDisconnected':
      return `Agent disconnected (${error.agent_id})`;
  }
}

/** Get a display label for the error category. */
function getErrorCategoryLabel(category: TaskError['category']): string {
  switch (category) {
    case 'timeout': return 'Timeout';
    case 'costCap': return 'Cost Cap Exceeded';
    case 'rateLimit': return 'Rate Limited';
    case 'executionError': return 'Execution Error';
    case 'agentDisconnected': return 'Agent Disconnected';
  }
}

function LoadingSkeleton() {
  return (
    <div className="animate-pulse space-y-6">
      <div className="h-6 w-64 bg-gray-200 rounded" />
      <div className="bg-white rounded-xl shadow-sm p-6 space-y-3">
        <div className="h-4 w-full bg-gray-200 rounded" />
        <div className="h-4 w-3/4 bg-gray-200 rounded" />
        <div className="h-4 w-1/2 bg-gray-200 rounded" />
      </div>
      <div className="bg-white rounded-xl shadow-sm p-6 space-y-3">
        <div className="h-4 w-48 bg-gray-200 rounded" />
        <div className="h-32 w-full bg-gray-200 rounded" />
      </div>
    </div>
  );
}

export default function TaskDetailView() {
  const { agentId, taskId } = useParams<{ agentId: string; taskId: string }>();
  const navigate = useNavigate();
  const { data: task, loading, error, refetch } = useApi<TaskDetail>(
    () => api.codingAgentTask(agentId!, taskId!),
    [agentId, taskId]
  );

  if (loading) {
    return <LoadingSkeleton />;
  }

  if (error) {
    return (
      <div className="bg-red-50 border border-red-200 rounded-lg p-4 flex items-center justify-between">
        <p className="text-sm text-red-700">Failed to load task: {error}</p>
        <button
          onClick={refetch}
          className="px-3 py-1 text-xs font-medium text-red-700 bg-red-100 rounded-lg hover:bg-red-200"
        >
          Retry
        </button>
      </div>
    );
  }

  if (!task) {
    return (
      <div className="text-center py-16">
        <p className="text-gray-500">Task not found.</p>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Header with back link, description, and outcome badge */}
      <div>
        <button
          onClick={() => navigate(`/ui/coding-agents/${agentId}`)}
          className="text-sm text-[var(--color-accent)] hover:underline mb-3 inline-flex items-center gap-1"
        >
          ← Back to History
        </button>
        <div className="flex items-start justify-between gap-4">
          <h3 className="text-lg font-semibold text-gray-900">{task.description}</h3>
          <OutcomeBadge outcome={task.outcome} />
        </div>
      </div>

      {/* Metadata grid */}
      <div className="bg-white rounded-xl shadow-sm p-6">
        <h4 className="text-sm font-semibold text-gray-700 mb-4">Task Details</h4>
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
          <div>
            <p className="text-xs text-gray-500 uppercase tracking-wide">Trigger Source</p>
            <p className="text-sm text-gray-900 mt-1">{formatTrigger(task.trigger)}</p>
          </div>
          <div>
            <p className="text-xs text-gray-500 uppercase tracking-wide">Start Time</p>
            <p className="text-sm text-gray-900 mt-1">{formatDateTime(task.started_at)}</p>
          </div>
          <div>
            <p className="text-xs text-gray-500 uppercase tracking-wide">End Time</p>
            <p className="text-sm text-gray-900 mt-1">
              {task.completed_at ? formatDateTime(task.completed_at) : '—'}
            </p>
          </div>
          <div>
            <p className="text-xs text-gray-500 uppercase tracking-wide">Duration</p>
            <p className="text-sm text-gray-900 mt-1">{formatDuration(task.duration_ms)}</p>
          </div>
        </div>
      </div>

      {/* Modified files section */}
      {task.modified_files.length > 0 && (
        <div className="bg-white rounded-xl shadow-sm p-6">
          <h4 className="text-sm font-semibold text-gray-700 mb-4">
            Modified Files ({task.modified_files.length})
          </h4>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-200">
                  <th className="text-left py-2 px-3 text-xs font-medium text-gray-500 uppercase">Path</th>
                  <th className="text-left py-2 px-3 text-xs font-medium text-gray-500 uppercase">Change</th>
                  <th className="text-right py-2 px-3 text-xs font-medium text-gray-500 uppercase">Added</th>
                  <th className="text-right py-2 px-3 text-xs font-medium text-gray-500 uppercase">Removed</th>
                </tr>
              </thead>
              <tbody>
                {task.modified_files.map((file) => (
                  <tr key={file.path} className="border-b border-gray-100">
                    <td className="py-2 px-3 font-mono text-xs text-gray-800">{file.path}</td>
                    <td className="py-2 px-3">
                      <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                        file.change_type === 'added'
                          ? 'bg-green-100 text-green-700'
                          : file.change_type === 'deleted'
                          ? 'bg-red-100 text-red-700'
                          : 'bg-blue-100 text-blue-700'
                      }`}>
                        {file.change_type}
                      </span>
                    </td>
                    <td className="py-2 px-3 text-right text-green-600 font-mono text-xs">
                      +{file.lines_added}
                    </td>
                    <td className="py-2 px-3 text-right text-red-600 font-mono text-xs">
                      -{file.lines_removed}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Output section */}
      <div className="bg-white rounded-xl shadow-sm p-6">
        <h4 className="text-sm font-semibold text-gray-700 mb-4">Output</h4>
        <pre className="bg-gray-900 text-gray-100 rounded-lg p-4 text-xs font-mono max-h-96 overflow-y-auto whitespace-pre-wrap">
          {task.output || '(no output)'}
        </pre>
      </div>

      {/* Token usage section */}
      {task.token_usage && (
        <div className="bg-white rounded-xl shadow-sm p-6">
          <h4 className="text-sm font-semibold text-gray-700 mb-4">Token Usage</h4>
          <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
            <div>
              <p className="text-xs text-gray-500 uppercase tracking-wide">Input Tokens</p>
              <p className="text-lg font-semibold text-gray-900 mt-1">
                {task.token_usage.input_tokens.toLocaleString()}
              </p>
            </div>
            <div>
              <p className="text-xs text-gray-500 uppercase tracking-wide">Output Tokens</p>
              <p className="text-lg font-semibold text-gray-900 mt-1">
                {task.token_usage.output_tokens.toLocaleString()}
              </p>
            </div>
            <div>
              <p className="text-xs text-gray-500 uppercase tracking-wide">Estimated Cost</p>
              <p className="text-lg font-semibold text-gray-900 mt-1">
                ${task.token_usage.estimated_cost_usd.toFixed(4)}
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Error section (only when outcome is failure and error is non-null) */}
      {task.outcome === 'failure' && task.error && (
        <div className="bg-red-50 border border-red-200 rounded-xl p-6">
          <h4 className="text-sm font-semibold text-red-800 mb-2">Error</h4>
          <div className="space-y-2">
            <p className="text-xs text-red-600 uppercase tracking-wide font-medium">
              {getErrorCategoryLabel(task.error.category)}
            </p>
            <p className="text-sm text-red-700">{getErrorMessage(task.error)}</p>
            {task.error.category === 'executionError' && task.error.partial_output && (
              <pre className="bg-red-100 rounded-lg p-3 text-xs font-mono text-red-800 max-h-48 overflow-y-auto whitespace-pre-wrap mt-3">
                {task.error.partial_output}
              </pre>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
