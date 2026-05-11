import { useState, useEffect } from 'react';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';

const MEMORY_BACKENDS = ['sqlrite', 'sqlite', 'postgres', 'none'] as const;
const VECTOR_STORES = ['qdrant', 'chroma', 'pinecone', 'none'] as const;
const EMBEDDING_PROVIDERS = ['openai', 'ollama', 'cohere', 'none'] as const;

interface SessionStatus {
  backend: string;
  healthy: boolean;
  connection_string: string;
}

export default function Settings() {
  // Memory
  const [memoryEnabled, setMemoryEnabled] = useState(false);
  const [memoryBackend, setMemoryBackend] = useState<string>('sqlrite');
  const [memoryEmbeddingProvider, setMemoryEmbeddingProvider] = useState<string>('openai');
  const [memoryEmbeddingModel, setMemoryEmbeddingModel] = useState('');

  // RAG
  const [ragEnabled, setRagEnabled] = useState(false);
  const [ragVectorStore, setRagVectorStore] = useState<string>('qdrant');
  const [ragEmbeddingProvider, setRagEmbeddingProvider] = useState<string>('openai');
  const [ragEmbeddingModel, setRagEmbeddingModel] = useState('');
  const [ragChunkSize, setRagChunkSize] = useState('512');

  // Session backend status
  const [sessionStatus, setSessionStatus] = useState<SessionStatus | null>(null);
  const [sessionStatusLoading, setSessionStatusLoading] = useState(true);

  const [saving, setSaving] = useState(false);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  useEffect(() => {
    const fetchSessionStatus = async () => {
      try {
        const res = await api.sessionStatus();
        if (res.ok && res.data) {
          setSessionStatus(res.data as SessionStatus);
        }
      } catch {
        // Silently fail — section will show as unavailable
      } finally {
        setSessionStatusLoading(false);
      }
    };
    fetchSessionStatus();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    setAlert(null);
    try {
      const res = await api.saveSettings({
        memory: {
          enabled: memoryEnabled,
          backend: memoryBackend,
          embedding_provider: memoryEmbeddingProvider,
          embedding_model: memoryEmbeddingModel,
        },
        rag: {
          enabled: ragEnabled,
          vector_store: ragVectorStore,
          embedding_provider: ragEmbeddingProvider,
          embedding_model: ragEmbeddingModel,
          chunk_size: parseInt(ragChunkSize, 10) || 512,
        },
      });
      if (res.ok) {
        setAlert({ type: 'success', message: 'Settings saved successfully.' });
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Settings</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      <div className="space-y-6 max-w-2xl">
        {/* Session Backend */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <h3 className="text-lg font-semibold mb-4">Session Backend</h3>
          {sessionStatusLoading ? (
            <div className="text-sm text-gray-400">Loading session status...</div>
          ) : sessionStatus ? (
            <div className="space-y-2">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-gray-700">Backend:</span>
                <span className="text-sm text-gray-600">{sessionStatus.backend}</span>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-gray-700">Status:</span>
                <span className={`inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full ${
                  sessionStatus.healthy
                    ? 'bg-green-50 text-green-700'
                    : 'bg-red-50 text-red-700'
                }`}>
                  <span className={`inline-block w-1.5 h-1.5 rounded-full ${
                    sessionStatus.healthy ? 'bg-green-500' : 'bg-red-500'
                  }`} />
                  {sessionStatus.healthy ? 'Healthy' : 'Unhealthy'}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-gray-700">Connection:</span>
                <span className="text-sm text-gray-500 font-mono">{sessionStatus.connection_string}</span>
              </div>
            </div>
          ) : (
            <div className="text-sm text-gray-400">Unable to fetch session status.</div>
          )}
        </div>

        {/* Memory Service */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">Memory Service</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={memoryEnabled}
                onChange={(e) => setMemoryEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {memoryEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Backend</label>
                <select
                  value={memoryBackend}
                  onChange={(e) => setMemoryBackend(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {MEMORY_BACKENDS.map((b) => <option key={b} value={b}>{b}</option>)}
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Embedding Provider</label>
                <select
                  value={memoryEmbeddingProvider}
                  onChange={(e) => setMemoryEmbeddingProvider(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {EMBEDDING_PROVIDERS.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Embedding Model</label>
                <input
                  type="text"
                  value={memoryEmbeddingModel}
                  onChange={(e) => setMemoryEmbeddingModel(e.target.value)}
                  placeholder="e.g. text-embedding-3-small"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
            </div>
          )}
        </div>

        {/* RAG Pipeline */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">RAG Pipeline</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={ragEnabled}
                onChange={(e) => setRagEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {ragEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Vector Store</label>
                <select
                  value={ragVectorStore}
                  onChange={(e) => setRagVectorStore(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {VECTOR_STORES.map((v) => <option key={v} value={v}>{v}</option>)}
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Embedding Provider</label>
                <select
                  value={ragEmbeddingProvider}
                  onChange={(e) => setRagEmbeddingProvider(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {EMBEDDING_PROVIDERS.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Embedding Model</label>
                <input
                  type="text"
                  value={ragEmbeddingModel}
                  onChange={(e) => setRagEmbeddingModel(e.target.value)}
                  placeholder="e.g. text-embedding-3-small"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Chunk Size</label>
                <input
                  type="number"
                  value={ragChunkSize}
                  onChange={(e) => setRagChunkSize(e.target.value)}
                  min="64"
                  max="8192"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
            </div>
          )}
        </div>

        {/* Save */}
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Settings'}
        </button>
      </div>
    </div>
  );
}
