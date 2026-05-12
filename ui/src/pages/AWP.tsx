import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import ConfirmDialog from '../components/ConfirmDialog';
import AlertBanner from '../components/AlertBanner';
import type { AwpSummary, AwpCapability, AwpSubscription } from '../types';
import { useState } from 'react';

export default function AWP() {
  const { data: summary, loading } = useApi<AwpSummary>(() => api.awpSummary(), []);
  const { data: capabilities } = useApi<AwpCapability[]>(() => api.awpCapabilities(), []);
  const { data: subscriptions, refetch: refetchSubs } = useApi<AwpSubscription[]>(() => api.awpSubscriptions(), []);
  const { data: consent } = useApi<unknown[]>(() => api.awpConsent(), []);

  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  const isEnabled = !!summary;

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

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">AWP (Agentic Web Protocol)</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Promotional banner when disabled */}
      {!isEnabled && (
        <div className="bg-gradient-to-r from-indigo-50 to-purple-50 border border-indigo-200 rounded-xl p-6 mb-6">
          <div className="flex items-start gap-4">
            <div className="text-4xl">🌐</div>
            <div className="flex-1">
              <h3 className="text-lg font-semibold text-gray-900 mb-2">Make Your Gateway Discoverable</h3>
              <p className="text-sm text-gray-600 mb-3">
                The Agentic Web Protocol (AWP) lets other AI agents discover and interact with your gateway.
                Enable AWP to expose capabilities, accept agent-to-agent messages, and participate in the agentic web.
              </p>
              <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 mb-4">
                <div className="bg-white/70 rounded-lg p-3">
                  <div className="text-sm font-medium text-gray-800">🔍 Discovery</div>
                  <div className="text-xs text-gray-500 mt-1">Other agents find you via <code className="bg-gray-100 px-1 rounded">/.well-known/awp.json</code></div>
                </div>
                <div className="bg-white/70 rounded-lg p-3">
                  <div className="text-sm font-medium text-gray-800">🤝 Agent-to-Agent</div>
                  <div className="text-xs text-gray-500 mt-1">Receive messages from other AI agents via <code className="bg-gray-100 px-1 rounded">/awp/a2a</code></div>
                </div>
                <div className="bg-white/70 rounded-lg p-3">
                  <div className="text-sm font-medium text-gray-800">📋 Capabilities</div>
                  <div className="text-xs text-gray-500 mt-1">Declare what your gateway can do in a machine-readable manifest</div>
                </div>
              </div>
              <div className="flex items-center gap-3">
                <div className="text-xs text-gray-500 bg-white/80 rounded-lg px-3 py-2 font-mono">
                  Add to config: <span className="text-indigo-600">{`"awp": { "enabled": true }`}</span>
                </div>
                <a
                  href="https://agenticwebprotocol.com"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-xs text-[var(--color-accent)] hover:underline font-medium"
                >
                  Learn more →
                </a>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Health & Site Info (when enabled) */}
      {isEnabled && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
          <div className="bg-white rounded-xl shadow-sm p-5">
            <h3 className="text-sm font-semibold text-gray-500 mb-2">Health</h3>
            <div className="flex items-center gap-3">
              <StatusBadge status={summary.health.state} />
              <span className="text-sm text-gray-600">{summary.health.message}</span>
            </div>
            <div className="text-xs text-gray-400 mt-2">Last updated: {new Date(summary.health.timestamp).toLocaleString()}</div>
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
      )}

      {/* Endpoints (always shown — shows what AWP provides) */}
      <h3 className="text-lg font-semibold mb-3">AWP Endpoints</h3>
      <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
        <table className="w-full">
          <thead>
            <tr className="bg-gray-50">
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Endpoint</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Method</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Description</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
            </tr>
          </thead>
          <tbody>
            {[
              { endpoint: '/.well-known/awp.json', method: 'GET', desc: 'Discovery document', always: true },
              { endpoint: '/awp/manifest', method: 'GET', desc: 'JSON-LD capability manifest', always: true },
              { endpoint: '/awp/health', method: 'GET', desc: 'Health state machine', always: true },
              { endpoint: '/awp/a2a', method: 'POST', desc: 'Agent-to-agent messages', always: false },
              { endpoint: '/awp/events/subscribe', method: 'POST', desc: 'Event subscriptions (HMAC-SHA256)', always: false },
              { endpoint: '/awp/consent/capture', method: 'POST', desc: 'Consent capture', always: false },
              { endpoint: '/awp/consent/check', method: 'GET', desc: 'Consent verification', always: false },
            ].map((ep) => (
              <tr key={ep.endpoint} className="border-t border-gray-100">
                <td className="px-4 py-3 text-sm font-mono">{ep.endpoint}</td>
                <td className="px-4 py-3 text-sm">{ep.method}</td>
                <td className="px-4 py-3 text-sm text-gray-600">{ep.desc}</td>
                <td className="px-4 py-3">
                  <StatusBadge status={isEnabled ? 'active' : 'disabled'} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Capabilities */}
      <h3 className="text-lg font-semibold mb-3">Capabilities</h3>
      {isEnabled && capabilities && capabilities.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Name</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Description</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Endpoint</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Access</th>
              </tr>
            </thead>
            <tbody>
              {capabilities.map((cap) => (
                <tr key={cap.name} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm font-medium">{cap.name}</td>
                  <td className="px-4 py-3 text-sm text-gray-600">{cap.description}</td>
                  <td className="px-4 py-3 text-sm font-mono text-gray-500">{cap.endpoint}</td>
                  <td className="px-4 py-3"><StatusBadge status={cap.access_level} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="text-center py-6 text-gray-400 bg-white rounded-xl shadow-sm mb-6">
          {isEnabled ? 'No capabilities registered — add them to business.toml' : 'Enable AWP to see registered capabilities'}
        </div>
      )}

      {/* Event Subscriptions */}
      <h3 className="text-lg font-semibold mb-3">Event Subscriptions</h3>
      {isEnabled && subscriptions && subscriptions.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Subscriber</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Callback URL</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Events</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {subscriptions.map((sub) => (
                <tr key={sub.id} className="border-t border-gray-100 hover:bg-gray-50">
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
        <div className="text-center py-6 text-gray-400 bg-white rounded-xl shadow-sm mb-6">
          {isEnabled ? 'No event subscriptions yet' : 'Enable AWP to manage subscriptions'}
        </div>
      )}

      {/* Consent Records */}
      <h3 className="text-lg font-semibold mb-3">Consent Records</h3>
      {isEnabled && consent && consent.length > 0 ? (
        <div className="bg-white rounded-xl shadow-sm p-4 mb-6">
          <pre className="text-sm font-mono text-gray-600 overflow-auto max-h-[300px]">
            {JSON.stringify(consent, null, 2)}
          </pre>
        </div>
      ) : (
        <div className="text-center py-6 text-gray-400 bg-white rounded-xl shadow-sm">
          {isEnabled ? 'No consent records' : 'Enable AWP to track consent'}
        </div>
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
