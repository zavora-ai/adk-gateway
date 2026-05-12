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

interface KgEntity {
  name: string;
  entity_type: string;
  observations: string[];
}

interface KgRelation {
  source: string;
  relation_type: string;
  target: string;
}

interface KgUser {
  user_id: string;
  entity_count: number;
  relation_count: number;
  entities: KgEntity[];
  relations: KgRelation[];
}

interface KgData {
  users: KgUser[];
}

export default function Memory() {
  const { data, loading, error } = useApi<MemoryData>(() => api.loadMemory(), []);
  const { data: kgData, loading: kgLoading } = useApi<KgData>(() => api.memoryEntities(), []);
  const [content, setContent] = useState('');
  const [savedContent, setSavedContent] = useState('');
  const [saving, setSaving] = useState(false);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [expandedUsers, setExpandedUsers] = useState<Set<string>>(new Set());

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

  const toggleUser = (userId: string) => {
    setExpandedUsers((prev) => {
      const next = new Set(prev);
      if (next.has(userId)) {
        next.delete(userId);
      } else {
        next.add(userId);
      }
      return next;
    });
  };

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

      {/* Knowledge Graph Browser */}
      <div className="bg-white rounded-xl shadow-sm p-6 mt-6">
        <h3 className="text-sm font-semibold text-gray-700 mb-4 flex items-center gap-2">
          <span>📊</span> Knowledge Graph Browser
        </h3>

        {kgLoading && <div className="text-gray-400 text-sm">Loading entities...</div>}

        {!kgLoading && kgData && kgData.users.length === 0 && (
          <div className="text-gray-400 text-sm">No knowledge graph data yet.</div>
        )}

        {!kgLoading && kgData && kgData.users.length > 0 && (
          <div className="space-y-2">
            {kgData.users.map((user) => (
              <div key={user.user_id} className="border border-gray-200 rounded-lg overflow-hidden">
                <button
                  onClick={() => toggleUser(user.user_id)}
                  className="w-full flex items-center justify-between px-4 py-3 bg-gray-50 hover:bg-gray-100 transition-colors text-left"
                >
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-sm font-medium text-gray-800">{user.user_id}</span>
                    <span className="text-xs text-gray-500">
                      ({user.entity_count} {user.entity_count === 1 ? 'entity' : 'entities'}, {user.relation_count} {user.relation_count === 1 ? 'relation' : 'relations'})
                    </span>
                  </div>
                  <span className="text-gray-400 text-sm">
                    {expandedUsers.has(user.user_id) ? '▼' : '▶'}
                  </span>
                </button>

                {expandedUsers.has(user.user_id) && (
                  <div className="px-4 py-3 space-y-3">
                    {user.entities.length === 0 && (
                      <div className="text-gray-400 text-xs">No entities.</div>
                    )}
                    {user.entities.map((entity) => (
                      <div key={entity.name} className="border-l-2 border-blue-300 pl-3">
                        <div className="flex items-center gap-2">
                          <span className="text-sm">🏷</span>
                          <span className="font-medium text-sm text-gray-800">{entity.name}</span>
                          <span className="text-xs text-gray-500 bg-gray-100 px-1.5 py-0.5 rounded">
                            {entity.entity_type}
                          </span>
                        </div>
                        {entity.observations.length > 0 && (
                          <ul className="mt-1 space-y-0.5 ml-6">
                            {entity.observations.map((obs, i) => (
                              <li key={i} className="text-xs text-gray-600">
                                <span className="text-gray-400 mr-1">-</span>
                                {obs}
                              </li>
                            ))}
                          </ul>
                        )}
                      </div>
                    ))}

                    {user.relations.length > 0 && (
                      <div className="mt-3 pt-3 border-t border-gray-100">
                        <div className="text-xs font-medium text-gray-500 mb-1">Relations</div>
                        {user.relations.map((rel, i) => (
                          <div key={i} className="text-xs text-gray-600 ml-2">
                            {rel.source} <span className="text-blue-500">—[{rel.relation_type}]→</span> {rel.target}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
