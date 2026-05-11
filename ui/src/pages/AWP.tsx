import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import ConfirmDialog from '../components/ConfirmDialog';
import AlertBanner from '../components/AlertBanner';
import type { AwpSummary, AwpCapability, AwpSubscription } from '../types';
import { useState } from 'react';

export default function AWP() {
  const { data: summary, loading, error } = useApi<AwpSummary>(() => api.awpSummary(), []);
  const { data: capabilities } = useApi<AwpCapability[]>(() => api.awpCapabilities(), []);
  const { data: subscriptions, refetch: refetchSubs } = useApi<AwpSubscription[]>(() => api.awpSubscriptions(), []);
  const { data: consent } = useApi<unknown[]>(() => api.awpConsent(), []);

  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  const handleDeleteSubscription = async (id: string) => {
    try {
      const res = await api.deleteAwpSubscription(id);
      if (res.ok) {
        setAlert({ type: 'success', message: 'Subscription deleted.' });
        refetchSubs();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to delete.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    }
    setDeleteTarget(null);
  };

  if (loading) return <div className="text-gray-400">Loading AWP...</div>;
  if (error) return <div className="text-red-600">Failed to load AWP: {error}</div>;

  // AWP disabled state
  if (!summary) {
    return (
      <div>
        <h2 className="text-2xl font-semibold mb-5">AWP (Agentic Web Protocol)</h2>
        <div className="bg-yellow-50 border border-yellow-200 rounded-xl p-6 text-center">
          <p className="text-yellow-800 mb-2">AWP is not enabled on this gateway.</p>
          <a
            href="/docs/awp-guide.md"
            target="_blank"
            rel="noopener noreferrer"
            className="text-[var(--color-accent)] hover:underline text-sm"
          >
            Learn how to enable AWP →
          </a>
        </div>
      </div>
    );
  }

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">AWP (Agent Web Protocol)</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Health & Site Info */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
        <div className="bg-white rounded-xl shadow-sm p-5">
          <h3 className="text-sm font-semibold text-gray-500 mb-2">Health</h3>
          <div className="flex items-center gap-3">
            <StatusBadge status={summary.health.state} />
            <span className="text-sm text-gray-600">{summary.health.message}</span>
          </div>
        </div>
        <div className="bg-white rounded-xl shadow-sm p-5">
          <h3 className="text-sm font-semibold text-gray-500 mb-2">Site Info</h3>
          <div className="text-sm space-y-1">
            <div><strong>Name:</strong> {summary.site.name}</div>
            <div><strong>Description:</strong> {summary.site.description}</div>
            <div><strong>Domain:</strong> <span className="font-mono">{summary.site.domain}</span></div>
          </div>
        </div>
      </div>

      {/* Capabilities */}
      <h3 className="text-lg font-semibold mb-3">Capabilities</h3>
      {capabilities && capabilities.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Name</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Description</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Endpoint</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Method</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Access</th>
              </tr>
            </thead>
            <tbody>
              {capabilities.map((cap) => (
                <tr key={cap.name} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-medium">{cap.name}</td>
                  <td className="px-4 py-3 text-sm text-gray-600">{cap.description}</td>
                  <td className="px-4 py-3 text-sm font-mono text-gray-500">{cap.endpoint}</td>
                  <td className="px-4 py-3 text-sm">{cap.method}</td>
                  <td className="px-4 py-3"><StatusBadge status={cap.access_level} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 mb-6">No capabilities registered</div>
      )}

      {/* Event Subscriptions */}
      <h3 className="text-lg font-semibold mb-3">Event Subscriptions</h3>
      {subscriptions && subscriptions.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Subscriber</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Callback URL</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Event Types</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {subscriptions.map((sub) => (
                <tr key={sub.id} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-mono">{sub.id}</td>
                  <td className="px-4 py-3 text-sm">{sub.subscriber}</td>
                  <td className="px-4 py-3 text-sm font-mono text-gray-500 break-all">{sub.callback_url}</td>
                  <td className="px-4 py-3 text-sm">{sub.event_types.join(', ')}</td>
                  <td className="px-4 py-3">
                    <button
                      onClick={() => setDeleteTarget(sub.id)}
                      className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400 mb-6">No event subscriptions</div>
      )}

      {/* Consent Records */}
      <h3 className="text-lg font-semibold mb-3">Consent Records</h3>
      {consent && consent.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm p-4 mb-6">
          <pre className="text-sm font-mono text-gray-600 overflow-auto max-h-[300px]">
            {JSON.stringify(consent, null, 2)}
          </pre>
        </div>
      ) : (
        <div className="text-center py-8 text-gray-400">No consent records</div>
      )}

      {deleteTarget && (
        <ConfirmDialog
          title="Delete Subscription"
          message={`Are you sure you want to delete subscription ${deleteTarget}?`}
          confirmLabel="Delete"
          destructive
          onConfirm={() => handleDeleteSubscription(deleteTarget)}
          onCancel={() => setDeleteTarget(null)}
        />
      )}
    </div>
  );
}
