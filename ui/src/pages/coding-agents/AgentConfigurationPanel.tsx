import { useState, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../../api/client';
import { useAgentDetail } from './AgentDetailLayout';
import type { AgentConfigUpdate } from '../../types';

interface FieldErrors {
  cost_cap_usd?: string;
  timeout_secs?: string;
  workspaces?: string;
}

export default function AgentConfigurationPanel() {
  const { agent, refetch } = useAgentDetail();
  const navigate = useNavigate();

  const [costCapUsd, setCostCapUsd] = useState<string>(
    agent.cost_cap_usd != null ? String(agent.cost_cap_usd) : '',
  );
  const [timeoutSecs, setTimeoutSecs] = useState<string>(String(agent.timeout_secs));
  const [workspaces, setWorkspaces] = useState<string>(agent.workspaces.join('\n'));

  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [successMessage, setSuccessMessage] = useState<string | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});

  const handleSave = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();

      setSaving(true);
      setSuccessMessage(null);
      setErrorMessage(null);
      setFieldErrors({});

      const config: AgentConfigUpdate = {
        cost_cap_usd: costCapUsd.trim() === '' ? null : Number(costCapUsd),
        timeout_secs: Number(timeoutSecs),
        workspaces: workspaces
          .split('\n')
          .map((w) => w.trim())
          .filter((w) => w.length > 0),
      };

      try {
        const result = await api.updateCodingAgent(agent.id, config);
        if (result.ok) {
          setSuccessMessage('Configuration saved successfully.');
          refetch();
        } else {
          // Check for field-level validation errors
          const msg = result.message || 'Failed to save configuration.';
          if (msg.toLowerCase().includes('cost_cap')) {
            setFieldErrors((prev) => ({ ...prev, cost_cap_usd: msg }));
          } else if (msg.toLowerCase().includes('timeout')) {
            setFieldErrors((prev) => ({ ...prev, timeout_secs: msg }));
          } else if (msg.toLowerCase().includes('workspace')) {
            setFieldErrors((prev) => ({ ...prev, workspaces: msg }));
          } else {
            setErrorMessage(msg);
          }
        }
      } catch {
        setErrorMessage('An unexpected error occurred. Please try again.');
      } finally {
        setSaving(false);
      }
    },
    [costCapUsd, timeoutSecs, workspaces, agent.id, refetch],
  );

  const handleDelete = useCallback(async () => {
    const confirmed = window.confirm(
      `Are you sure you want to delete agent "${agent.alias || agent.id}"? This action cannot be undone.`,
    );
    if (!confirmed) return;

    setDeleting(true);
    setErrorMessage(null);

    try {
      const result = await api.deleteCodingAgent(agent.id);
      if (result.ok) {
        navigate('/ui/coding-agents');
      } else {
        setErrorMessage(result.message || 'Failed to delete agent.');
      }
    } catch {
      setErrorMessage('An unexpected error occurred while deleting the agent.');
    } finally {
      setDeleting(false);
    }
  }, [agent.id, agent.alias, navigate]);

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Agent Configuration</h3>

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

      <form onSubmit={handleSave} className="space-y-4">
        {/* Cost Cap (USD) */}
        <div>
          <label htmlFor="cost-cap" className="block text-sm font-medium text-gray-700 mb-1">
            Per-Task Cost Cap (USD){' '}
            <span className="text-gray-400 font-normal">(leave empty for no cap)</span>
          </label>
          <input
            id="cost-cap"
            type="number"
            step="0.01"
            min="0"
            value={costCapUsd}
            onChange={(e) => setCostCapUsd(e.target.value)}
            placeholder="No cap"
            className={`w-full px-3 py-2 border rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] ${
              fieldErrors.cost_cap_usd ? 'border-red-300' : 'border-gray-300'
            }`}
          />
          {fieldErrors.cost_cap_usd && (
            <p className="mt-1 text-xs text-red-600">{fieldErrors.cost_cap_usd}</p>
          )}
        </div>

        {/* Timeout (seconds) */}
        <div>
          <label htmlFor="timeout-secs" className="block text-sm font-medium text-gray-700 mb-1">
            Task Timeout (seconds)
          </label>
          <input
            id="timeout-secs"
            type="number"
            step="1"
            min="1"
            value={timeoutSecs}
            onChange={(e) => setTimeoutSecs(e.target.value)}
            className={`w-full px-3 py-2 border rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] ${
              fieldErrors.timeout_secs ? 'border-red-300' : 'border-gray-300'
            }`}
          />
          {fieldErrors.timeout_secs && (
            <p className="mt-1 text-xs text-red-600">{fieldErrors.timeout_secs}</p>
          )}
        </div>

        {/* Workspaces */}
        <div>
          <label htmlFor="workspaces" className="block text-sm font-medium text-gray-700 mb-1">
            Workspace Directories{' '}
            <span className="text-gray-400 font-normal">(one path per line)</span>
          </label>
          <textarea
            id="workspaces"
            value={workspaces}
            onChange={(e) => setWorkspaces(e.target.value)}
            rows={4}
            placeholder="/home/user/project"
            className={`w-full px-3 py-2 border rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] ${
              fieldErrors.workspaces ? 'border-red-300' : 'border-gray-300'
            }`}
          />
          {fieldErrors.workspaces && (
            <p className="mt-1 text-xs text-red-600">{fieldErrors.workspaces}</p>
          )}
        </div>

        {/* Save Button */}
        <div className="pt-2">
          <button
            type="submit"
            disabled={saving}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {saving ? 'Saving...' : 'Save Changes'}
          </button>
        </div>
      </form>

      {/* Danger Zone */}
      <div className="mt-8 pt-6 border-t border-gray-200">
        <h4 className="text-sm font-semibold text-gray-700 mb-2">Danger Zone</h4>
        <p className="text-xs text-gray-500 mb-3">
          Permanently remove this agent and all associated data.
        </p>
        <button
          onClick={handleDelete}
          disabled={deleting}
          className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {deleting ? 'Deleting...' : 'Delete Agent'}
        </button>
      </div>
    </div>
  );
}
