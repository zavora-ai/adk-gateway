import { useState, useCallback } from 'react';
import { api } from '../../api/client';
import { useAgentDetail } from './AgentDetailLayout';
import type { TaskDelegationPayload } from '../../types';

export default function TaskDelegationForm() {
  const { agent } = useAgentDetail();
  const agentId = agent.id;

  const [description, setDescription] = useState('');
  const [workspace, setWorkspace] = useState<string>(agent.workspaces[0] ?? '');
  const [fileContext, setFileContext] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [successMessage, setSuccessMessage] = useState<string | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const isAgentUnavailable =
    agent.connection_status === 'disconnected' || agent.connection_status === 'error';

  const isDescriptionValid = description.trim().length > 0;
  const isSubmitDisabled = !isDescriptionValid || isAgentUnavailable || submitting;

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();

      if (!isDescriptionValid || isAgentUnavailable) return;

      setSubmitting(true);
      setSuccessMessage(null);
      setErrorMessage(null);

      const filePaths = fileContext
        .split(',')
        .map((f) => f.trim())
        .filter((f) => f.length > 0);

      const payload: TaskDelegationPayload = {
        description: description.trim(),
        workspace: workspace || null,
        file_context: filePaths.length > 0 ? filePaths : null,
      };

      try {
        const result = await api.delegateTask(agentId, payload);
        if (result.ok) {
          setSuccessMessage('Task delegated successfully.');
          setDescription('');
          setWorkspace(agent.workspaces[0] ?? '');
          setFileContext('');
        } else {
          setErrorMessage(result.message || 'Failed to delegate task.');
        }
      } catch {
        setErrorMessage('An unexpected error occurred. Please try again.');
      } finally {
        setSubmitting(false);
      }
    },
    [description, workspace, fileContext, agentId, agent.workspaces, isDescriptionValid, isAgentUnavailable],
  );

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Delegate New Task</h3>

      {isAgentUnavailable && (
        <div className="mb-4 px-4 py-3 bg-yellow-50 border border-yellow-200 rounded-lg">
          <p className="text-sm text-yellow-800">
            Agent is currently unavailable ({agent.connection_status}). Task submission is disabled.
          </p>
        </div>
      )}

      {successMessage && (
        <div className="mb-4 px-4 py-3 bg-green-50 border border-green-200 rounded-lg flex items-center justify-between">
          <p className="text-sm text-green-700">{successMessage}</p>
          <button
            onClick={() => setSuccessMessage(null)}
            className="text-green-700 hover:text-green-900 text-sm font-medium"
          >
            ✕
          </button>
        </div>
      )}

      {errorMessage && (
        <div className="mb-4 px-4 py-3 bg-red-50 border border-red-200 rounded-lg flex items-center justify-between">
          <p className="text-sm text-red-700">{errorMessage}</p>
          <button
            onClick={() => setErrorMessage(null)}
            className="text-red-700 hover:text-red-900 text-sm font-medium"
          >
            ✕
          </button>
        </div>
      )}

      <form onSubmit={handleSubmit} className="space-y-4">
        {/* Task Description */}
        <div>
          <label htmlFor="task-description" className="block text-sm font-medium text-gray-700 mb-1">
            Task Description <span className="text-red-500">*</span>
          </label>
          <textarea
            id="task-description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={6}
            placeholder="Describe the task for the coding agent..."
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          />
        </div>

        {/* Workspace Selector */}
        <div>
          <label htmlFor="workspace-select" className="block text-sm font-medium text-gray-700 mb-1">
            Workspace
          </label>
          <select
            id="workspace-select"
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          >
            {agent.workspaces.map((ws) => (
              <option key={ws} value={ws}>
                {ws}
              </option>
            ))}
          </select>
        </div>

        {/* File Context */}
        <div>
          <label htmlFor="file-context" className="block text-sm font-medium text-gray-700 mb-1">
            File Context <span className="text-gray-400 font-normal">(optional, comma-separated paths)</span>
          </label>
          <input
            id="file-context"
            type="text"
            value={fileContext}
            onChange={(e) => setFileContext(e.target.value)}
            placeholder="src/main.ts, src/utils.ts"
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          />
        </div>

        {/* Submit Button */}
        <div className="pt-2">
          <button
            type="submit"
            disabled={isSubmitDisabled}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {submitting ? 'Delegating...' : 'Delegate Task'}
          </button>
        </div>
      </form>
    </div>
  );
}
