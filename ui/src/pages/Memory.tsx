import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';
import MetricCard from '../components/MetricCard';
import { useState, useEffect, useCallback } from 'react';

interface MemoryData {
  content: string;
  path: string;
  exists: boolean;
  stats?: {
    backend: string;
    embedding: string;
    total_users: number;
    total_entities: number;
    total_relations: number;
    total_observations: number;
    per_user?: { user_id: string; entities: number; relations: number; observations: number }[];
  };
}

export default function Memory() {
  const { data, loading, error } = useApi<MemoryData>(() => api.loadMemory(), []);
  const [content, setContent] = useState('');
  const [savedContent, setSavedContent] = useState('');
  const [saving, setSaving] = useState(false);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  useEffect(() => {
    if (data) {
      setContent(data.content || '');
      setSavedContent(data.content || '');
    }
  }, [data]);

  const hasUnsavedChanges = content !== savedContent;

  const handleSave = useCallback(async () => {
    setSaving(true);
    setAlert(null);
    try {
      const res = await api.saveMemory(content);
      if (res.ok) {
        setSavedContent(content);
        setAlert({ type: 'success', message: 'Memory protocol saved.' });
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  }, [content]);

  // Cmd+S keyboard shortcut
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault();
        handleSave();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [handleSave]);

  if (loading) return <div className="text-gray-400">Loading memory...</div>;
  if (error) return <div className="text-red-600">Failed to load memory: {error}</div>;

  const stats = data?.stats;

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Memory</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {/* Stats cards */}
      {stats && (
        <div className="flex flex-wrap gap-4 mb-6">
          <MetricCard label="Backend" value={stats.backend} />
          <MetricCard label="Embedding" value={stats.embedding} />
          <MetricCard label="Users" value={stats.total_users} />
          <MetricCard label="Entities" value={stats.total_entities} />
          <MetricCard label="Relations" value={stats.total_relations} />
          <MetricCard label="Observations" value={stats.total_observations} />
        </div>
      )}

      {/* KG stats per user */}
      {stats?.per_user && stats.per_user.length > 0 && (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <h3 className="text-sm font-semibold px-4 py-3 bg-gray-50">KG Stats per User</h3>
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-2 text-xs uppercase tracking-wide text-gray-500">User ID</th>
                <th className="text-left px-4 py-2 text-xs uppercase tracking-wide text-gray-500">Entities</th>
                <th className="text-left px-4 py-2 text-xs uppercase tracking-wide text-gray-500">Relations</th>
                <th className="text-left px-4 py-2 text-xs uppercase tracking-wide text-gray-500">Observations</th>
              </tr>
            </thead>
            <tbody>
              {stats.per_user.map((u) => (
                <tr key={u.user_id} className="border-t border-gray-100">
                  <td className="px-4 py-2 text-sm font-mono">{u.user_id}</td>
                  <td className="px-4 py-2 text-sm">{u.entities}</td>
                  <td className="px-4 py-2 text-sm">{u.relations}</td>
                  <td className="px-4 py-2 text-sm">{u.observations}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Memory protocol editor */}
      <div className="bg-white rounded-xl shadow-sm p-6">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-3">
            <h3 className="text-sm font-semibold text-gray-700">Memory Protocol</h3>
            {hasUnsavedChanges && (
              <span className="text-xs text-yellow-600 bg-yellow-50 px-2 py-0.5 rounded-full font-medium">
                Unsaved changes
              </span>
            )}
          </div>
          <div className="flex items-center gap-3">
            <span className="text-xs text-gray-400">{content.length} chars</span>
            <button
              onClick={handleSave}
              disabled={saving || !hasUnsavedChanges}
              className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
            >
              {saving ? 'Saving...' : 'Save (⌘S)'}
            </button>
          </div>
        </div>

        {data?.path && (
          <div className="text-xs text-gray-400 mb-2">File: {data.path}</div>
        )}

        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          className="w-full h-[500px] font-mono text-sm p-4 border border-gray-700 rounded-lg bg-gray-900 text-green-400 focus:outline-none focus:border-[var(--color-accent)]"
          spellCheck={false}
          placeholder="# Memory Protocol&#10;&#10;Write your memory protocol here..."
        />
      </div>
    </div>
  );
}
