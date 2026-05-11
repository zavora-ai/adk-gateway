import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import ConfirmDialog from './ConfirmDialog';
import AlertBanner from './AlertBanner';
import type { DelegationRule, AgentRecord } from '../types';

interface Props {
  agents: AgentRecord[];
}

/**
 * Client-side cycle detection: checks if adding caller→target would create
 * a circular chain in the existing permissions graph.
 */
export function wouldCreateCycle(
  permissions: DelegationRule[],
  caller: string,
  target: string,
): boolean {
  if (caller === target) return true;

  // BFS from target: can we reach caller?
  const visited = new Set<string>();
  const queue: string[] = [target];

  while (queue.length > 0) {
    const node = queue.shift()!;
    if (node === caller) return true;
    if (visited.has(node)) continue;
    visited.add(node);

    for (const rule of permissions) {
      if (rule.caller_id === node) {
        queue.push(rule.target_id);
      }
    }
  }

  return false;
}

export default function DelegationPermissions({ agents }: Props) {
  const [permissions, setPermissions] = useState<DelegationRule[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  // Add form state
  const [callerId, setCallerId] = useState('');
  const [targetId, setTargetId] = useState('');
  const [cycleWarning, setCycleWarning] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  // Remove confirmation
  const [removeTarget, setRemoveTarget] = useState<DelegationRule | null>(null);

  const fetchPermissions = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.delegationList();
      if (res.ok && res.data) {
        setPermissions(res.data);
      } else {
        setError(res.message || 'Failed to load delegation permissions');
      }
    } catch {
      setError('Network error loading delegation permissions');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPermissions();
  }, [fetchPermissions]);

  // Client-side cycle detection preview
  useEffect(() => {
    if (callerId && targetId) {
      if (wouldCreateCycle(permissions, callerId, targetId)) {
        setCycleWarning(
          `Adding ${callerId} → ${targetId} would create a circular delegation chain`
        );
      } else {
        setCycleWarning(null);
      }
    } else {
      setCycleWarning(null);
    }
  }, [callerId, targetId, permissions]);

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!callerId || !targetId) return;
    if (cycleWarning) return;

    setAdding(true);
    try {
      const res = await api.delegationAdd(callerId, targetId);
      if (res.ok) {
        setAlert({ type: 'success', message: `Delegation added: ${callerId} → ${targetId}` });
        setCallerId('');
        setTargetId('');
        fetchPermissions();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to add delegation' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error' });
    } finally {
      setAdding(false);
    }
  };

  const handleRemove = async (rule: DelegationRule) => {
    try {
      const res = await api.delegationRemove(rule.caller_id, rule.target_id);
      if (res.ok) {
        setAlert({ type: 'success', message: `Delegation removed: ${rule.caller_id} → ${rule.target_id}` });
        fetchPermissions();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to remove delegation' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error' });
    }
    setRemoveTarget(null);
  };

  if (loading) return <div className="text-gray-400">Loading delegation permissions...</div>;
  if (error) return <div className="text-red-600">{error}</div>;

  return (
    <div className="mt-8">
      <h3 className="text-lg font-semibold mb-4">Delegation Permissions</h3>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Add Permission Form */}
      <form onSubmit={handleAdd} className="bg-white rounded-xl shadow-sm p-4 mb-4 max-w-xl">
        <h4 className="text-sm font-medium text-gray-700 mb-3">Add Permission</h4>
        <div className="flex gap-3 items-end">
          <div className="flex-1">
            <label className="block text-xs text-gray-500 mb-1">Caller Agent</label>
            <select
              value={callerId}
              onChange={(e) => setCallerId(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              aria-label="Caller agent"
            >
              <option value="">Select caller...</option>
              {agents.map((a) => (
                <option key={a.id} value={a.id}>{a.name} ({a.id})</option>
              ))}
            </select>
          </div>
          <div className="text-gray-400 pb-2">→</div>
          <div className="flex-1">
            <label className="block text-xs text-gray-500 mb-1">Target Agent</label>
            <select
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              aria-label="Target agent"
            >
              <option value="">Select target...</option>
              {agents.map((a) => (
                <option key={a.id} value={a.id}>{a.name} ({a.id})</option>
              ))}
            </select>
          </div>
          <button
            type="submit"
            disabled={adding || !callerId || !targetId || !!cycleWarning}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
          >
            {adding ? 'Adding...' : 'Add'}
          </button>
        </div>
        {cycleWarning && (
          <div className="mt-2 text-sm text-red-600 flex items-center gap-1" role="alert">
            <span>⚠</span> {cycleWarning}
          </div>
        )}
      </form>

      {/* Permissions List */}
      {permissions.length === 0 ? (
        <div className="text-center py-8 text-gray-400">No delegation permissions configured</div>
      ) : (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Caller</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500"></th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Target</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Created</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {permissions.map((rule, i) => (
                <tr key={i} className="border-t border-gray-100">
                  <td className="px-4 py-3 text-sm font-mono">{rule.caller_id}</td>
                  <td className="px-4 py-3 text-gray-400">→</td>
                  <td className="px-4 py-3 text-sm font-mono">{rule.target_id}</td>
                  <td className="px-4 py-3 text-sm text-gray-500">
                    {rule.created_at ? new Date(rule.created_at).toLocaleDateString() : '—'}
                  </td>
                  <td className="px-4 py-3">
                    <button
                      onClick={() => setRemoveTarget(rule)}
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
      )}

      {/* Remove confirmation */}
      {removeTarget && (
        <ConfirmDialog
          title="Remove Delegation Permission"
          message={`Remove delegation permission: ${removeTarget.caller_id} → ${removeTarget.target_id}?`}
          confirmLabel="Remove"
          destructive
          onConfirm={() => handleRemove(removeTarget)}
          onCancel={() => setRemoveTarget(null)}
        />
      )}
    </div>
  );
}
