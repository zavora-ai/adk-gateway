import { useState, useEffect } from 'react';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';
import type {
  ApprovalConfig,
  StaleContextConfig,
  RateLimitConfig,
  HealthMonitorConfig,
  EncryptionStatus,
  SensitiveField,
  LogRotationConfig,
  SystemInfo,
} from '../types';

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
        // Silently fail
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
                  sessionStatus.healthy ? 'bg-green-50 text-green-700' : 'bg-red-50 text-red-700'
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

        {/* Save Memory/RAG */}
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Settings'}
        </button>

        {/* Tool Approval (Task 14) */}
        <ToolApprovalSettings />

        {/* Session & Context (Task 15) */}
        <StaleContextSettings />

        {/* Rate Limiting (Task 16) */}
        <RateLimitSettings />

        {/* Health Monitoring (Task 18) */}
        <HealthMonitorSettings />

        {/* Security / Encryption (Task 20) */}
        <EncryptionSettings />

        {/* Logging (Task 21) */}
        <LogRotationSettings />

        {/* Deployment (Task 22) */}
        <DeploymentSettings />
      </div>
    </div>
  );
}

// ── Tool Approval Settings (Task 14) ──────────────────────────────

function ToolApprovalSettings() {
  const [config, setConfig] = useState<ApprovalConfig>({
    enabled: true,
    require_approval: ['fs_write', 'fs_delete', 'shell_exec', 'run_command'],
    timeout_secs: 120,
  });
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [newTool, setNewTool] = useState('');

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getApprovalConfig();
        if (res.ok && res.data) {
          setConfig(res.data);
        }
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    await api.saveApprovalConfig(config);
    setSaving(false);
  };

  const addTool = () => {
    if (newTool.trim() && !config.require_approval.includes(newTool.trim())) {
      setConfig({ ...config, require_approval: [...config.require_approval, newTool.trim()] });
      setNewTool('');
    }
  };

  const removeTool = (tool: string) => {
    setConfig({ ...config, require_approval: config.require_approval.filter((t) => t !== tool) });
  };

  if (!loaded) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-lg font-semibold">Tool Approval</h3>
        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={config.enabled}
            onChange={(e) => setConfig({ ...config, enabled: e.target.checked })}
            className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
          />
          <span className="text-sm text-gray-600">Enabled</span>
        </label>
      </div>

      {config.enabled && (
        <div className="space-y-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-2">Tools Requiring Approval</label>
            <div className="flex flex-wrap gap-2 mb-2">
              {config.require_approval.map((tool) => (
                <span key={tool} className="inline-flex items-center gap-1 px-2.5 py-1 bg-orange-50 text-orange-700 rounded-lg text-xs font-medium">
                  {tool}
                  <button onClick={() => removeTool(tool)} className="text-orange-400 hover:text-orange-600 ml-1">×</button>
                </span>
              ))}
            </div>
            <div className="flex gap-2">
              <input
                type="text"
                value={newTool}
                onChange={(e) => setNewTool(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && (e.preventDefault(), addTool())}
                placeholder="Add tool name..."
                className="flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
              />
              <button onClick={addTool} className="px-3 py-2 text-sm font-medium bg-gray-100 rounded-lg hover:bg-gray-200">Add</button>
            </div>
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">
              Timeout: {config.timeout_secs}s
            </label>
            <input
              type="range"
              min="30"
              max="300"
              step="10"
              value={config.timeout_secs}
              onChange={(e) => setConfig({ ...config, timeout_secs: parseInt(e.target.value, 10) })}
              className="w-full"
            />
            <div className="flex justify-between text-xs text-gray-400">
              <span>30s</span>
              <span>300s</span>
            </div>
          </div>

          <button
            onClick={handleSave}
            disabled={saving}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
          >
            {saving ? 'Saving...' : 'Save Approval Config'}
          </button>
        </div>
      )}
    </div>
  );
}

// ── Stale Context Settings (Task 15) ──────────────────────────────

function StaleContextSettings() {
  const [config, setConfig] = useState<StaleContextConfig>({ idle_threshold_hours: 4 });
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getStaleContextConfig();
        if (res.ok && res.data) setConfig(res.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    await api.saveStaleContextConfig(config);
    setSaving(false);
  };

  if (!loaded) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Session & Context</h3>
      <div className="space-y-3">
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Idle Threshold (hours)</label>
          <input
            type="number"
            min="1"
            max="168"
            value={config.idle_threshold_hours}
            onChange={(e) => setConfig({ idle_threshold_hours: parseInt(e.target.value, 10) || 4 })}
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          />
          <p className="text-xs text-gray-400 mt-1">
            After this many hours of inactivity, a welcome-back message will be sent on the next interaction.
          </p>
        </div>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Context Config'}
        </button>
      </div>
    </div>
  );
}

// ── Rate Limit Settings (Task 16) ─────────────────────────────────

function RateLimitSettings() {
  const [config, setConfig] = useState<RateLimitConfig>({
    max_calls: 10,
    window_secs: 5,
    cooldown_secs: 3,
    max_triggers: 3,
  });
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getRateLimitConfig();
        if (res.ok && res.data) setConfig(res.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    await api.saveRateLimitConfig(config);
    setSaving(false);
  };

  if (!loaded) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Rate Limiting</h3>
      <div className="space-y-3">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Max Calls per Window</label>
            <input
              type="number"
              min="1"
              max="100"
              value={config.max_calls}
              onChange={(e) => setConfig({ ...config, max_calls: parseInt(e.target.value, 10) || 10 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Window Duration (seconds)</label>
            <input
              type="number"
              min="1"
              max="60"
              value={config.window_secs}
              onChange={(e) => setConfig({ ...config, window_secs: parseInt(e.target.value, 10) || 5 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Cooldown Duration (seconds)</label>
            <input
              type="number"
              min="1"
              max="30"
              value={config.cooldown_secs}
              onChange={(e) => setConfig({ ...config, cooldown_secs: parseInt(e.target.value, 10) || 3 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Max Triggers Before Termination</label>
            <input
              type="number"
              min="1"
              max="10"
              value={config.max_triggers}
              onChange={(e) => setConfig({ ...config, max_triggers: parseInt(e.target.value, 10) || 3 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
        </div>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Rate Limit Config'}
        </button>
      </div>
    </div>
  );
}

// ── Health Monitor Settings (Task 18) ─────────────────────────────

function HealthMonitorSettings() {
  const [config, setConfig] = useState<HealthMonitorConfig>({
    check_interval_secs: 60,
    failure_threshold: 3,
    alert_webhook_url: '',
    alert_telegram_admin: '',
  });
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getHealthMonitorConfig();
        if (res.ok && res.data) setConfig(res.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    await api.saveHealthMonitorConfig(config);
    setSaving(false);
  };

  if (!loaded) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Health Monitoring</h3>
      <div className="space-y-3">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Check Interval (seconds)</label>
            <input
              type="number"
              min="10"
              max="600"
              value={config.check_interval_secs}
              onChange={(e) => setConfig({ ...config, check_interval_secs: parseInt(e.target.value, 10) || 60 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Failure Threshold</label>
            <input
              type="number"
              min="1"
              max="10"
              value={config.failure_threshold}
              onChange={(e) => setConfig({ ...config, failure_threshold: parseInt(e.target.value, 10) || 3 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
        </div>
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Alert Webhook URL</label>
          <input
            type="url"
            value={config.alert_webhook_url}
            onChange={(e) => setConfig({ ...config, alert_webhook_url: e.target.value })}
            placeholder="https://hooks.example.com/alert"
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          />
        </div>
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Alert Telegram Admin Chat ID</label>
          <input
            type="text"
            value={config.alert_telegram_admin}
            onChange={(e) => setConfig({ ...config, alert_telegram_admin: e.target.value })}
            placeholder="e.g. 123456789"
            className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
          />
        </div>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Health Config'}
        </button>
      </div>
    </div>
  );
}

// ── Encryption Settings (Task 20) ─────────────────────────────────

function EncryptionSettings() {
  const [status, setStatus] = useState<EncryptionStatus | null>(null);
  const [fields, setFields] = useState<SensitiveField[]>([]);
  const [keyPath, setKeyPath] = useState('');
  const [saving, setSaving] = useState(false);
  const [encrypting, setEncrypting] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const [statusRes, fieldsRes] = await Promise.all([
          api.getEncryptionStatus(),
          api.getSensitiveFields(),
        ]);
        if (statusRes.ok && statusRes.data) {
          setStatus(statusRes.data);
          setKeyPath(statusRes.data.key_file_path || '');
        }
        if (fieldsRes.ok && fieldsRes.data) setFields(fieldsRes.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSaveKeyPath = async () => {
    setSaving(true);
    await api.saveEncryptionKeyPath(keyPath);
    setSaving(false);
  };

  const handleEncryptAll = async () => {
    setEncrypting(true);
    try {
      await api.encryptAll();
      // Refresh fields
      const res = await api.getSensitiveFields();
      if (res.ok && res.data) setFields(res.data);
    } catch { /* error handled silently */ }
    setEncrypting(false);
  };

  if (!loaded) return null;

  const hasPlaintext = fields.some((f) => !f.encrypted);
  const noKey = status && !status.key_configured;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Security</h3>

      {/* Warning banner */}
      {status && status.enabled && noKey && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-3 mb-4">
          <p className="text-sm text-red-700 font-medium">
            ⚠️ Encrypted values exist but no decryption key is configured. The gateway will fail to start.
          </p>
        </div>
      )}

      <div className="space-y-4">
        <div className="flex items-center gap-2 text-sm">
          <span className="text-gray-600">Encryption:</span>
          <span className={`font-medium ${status?.enabled ? 'text-green-700' : 'text-gray-500'}`}>
            {status?.enabled ? 'Enabled' : 'Disabled'}
          </span>
        </div>

        {/* Sensitive fields list */}
        {fields.length > 0 && (
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-2">Sensitive Fields</label>
            <div className="space-y-1">
              {fields.map((field) => (
                <div key={field.field_name} className="flex items-center justify-between px-3 py-2 bg-gray-50 rounded-lg">
                  <span className="text-sm font-mono text-gray-700">{field.field_name}</span>
                  <span className={`text-xs font-medium ${field.encrypted ? 'text-green-700' : 'text-orange-600'}`}>
                    {field.encrypted ? '🔒 Encrypted' : '⚠️ Plaintext'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Encrypt All button */}
        {hasPlaintext && (
          <button
            onClick={handleEncryptAll}
            disabled={encrypting}
            className="px-4 py-2 text-sm font-medium text-white bg-orange-600 rounded-lg hover:bg-orange-700 disabled:opacity-50"
          >
            {encrypting ? 'Encrypting...' : '🔐 Encrypt All Plaintext Fields'}
          </button>
        )}

        {/* Key file path */}
        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">Encryption Key File Path</label>
          <div className="flex gap-2">
            <input
              type="text"
              value={keyPath}
              onChange={(e) => setKeyPath(e.target.value)}
              placeholder="/etc/adk-gateway/encryption.key"
              className="flex-1 px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono focus:outline-none focus:border-[var(--color-accent)]"
            />
            <button
              onClick={handleSaveKeyPath}
              disabled={saving}
              className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
            >
              {saving ? '...' : 'Save'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Log Rotation Settings (Task 21) ───────────────────────────────

function LogRotationSettings() {
  const [config, setConfig] = useState<LogRotationConfig>({
    rotation_policy: 'daily',
    retention_days: 7,
    max_file_size_mb: 100,
    format: 'json',
  });
  const [saving, setSaving] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getLogRotationConfig();
        if (res.ok && res.data) setConfig(res.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  const handleSave = async () => {
    setSaving(true);
    await api.saveLogRotationConfig(config);
    setSaving(false);
  };

  if (!loaded) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Logging</h3>
      <div className="space-y-3">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Rotation Policy</label>
            <select
              value={config.rotation_policy}
              onChange={(e) => setConfig({ ...config, rotation_policy: e.target.value as LogRotationConfig['rotation_policy'] })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            >
              <option value="daily">Daily</option>
              <option value="hourly">Hourly</option>
              <option value="size">Size-based</option>
            </select>
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Format</label>
            <select
              value={config.format}
              onChange={(e) => setConfig({ ...config, format: e.target.value as 'json' | 'pretty' })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            >
              <option value="json">JSON</option>
              <option value="pretty">Pretty</option>
            </select>
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Retention Period (days)</label>
            <input
              type="number"
              min="1"
              max="365"
              value={config.retention_days}
              onChange={(e) => setConfig({ ...config, retention_days: parseInt(e.target.value, 10) || 7 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">Max File Size (MB)</label>
            <input
              type="number"
              min="1"
              max="1000"
              value={config.max_file_size_mb}
              onChange={(e) => setConfig({ ...config, max_file_size_mb: parseInt(e.target.value, 10) || 100 })}
              className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
        </div>
        <button
          onClick={handleSave}
          disabled={saving}
          className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Logging Config'}
        </button>
      </div>
    </div>
  );
}

// ── Deployment Settings (Task 22) ─────────────────────────────────

function DeploymentSettings() {
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const res = await api.getSystemInfo();
        if (res.ok && res.data) setSystemInfo(res.data);
      } catch { /* use defaults */ }
      setLoaded(true);
    })();
  }, []);

  if (!loaded || !systemInfo) return null;

  return (
    <div className="bg-white rounded-xl shadow-sm p-6">
      <h3 className="text-lg font-semibold mb-4">Deployment</h3>
      <div className="space-y-3">
        <div className="grid grid-cols-2 gap-4">
          <div>
            <span className="text-sm font-medium text-gray-700">Version</span>
            <div className="text-sm font-mono text-gray-600">{systemInfo.version}</div>
          </div>
          <div>
            <span className="text-sm font-medium text-gray-700">Drain Timeout</span>
            <div className="text-sm text-gray-600">{systemInfo.drain_timeout_secs}s</div>
          </div>
          <div>
            <span className="text-sm font-medium text-gray-700">Config Path</span>
            <div className="text-xs font-mono text-gray-500 truncate" title={systemInfo.config_path}>{systemInfo.config_path}</div>
          </div>
          {systemInfo.docker_status && (
            <div>
              <span className="text-sm font-medium text-gray-700">Docker</span>
              <div className="text-sm text-gray-600">{systemInfo.docker_status}</div>
            </div>
          )}
          {systemInfo.systemd_status && (
            <div>
              <span className="text-sm font-medium text-gray-700">Systemd</span>
              <div className="text-sm text-gray-600">{systemInfo.systemd_status}</div>
            </div>
          )}
        </div>
        {systemInfo.build_features && systemInfo.build_features.length > 0 && (
          <div>
            <span className="text-sm font-medium text-gray-700">Build Features</span>
            <div className="flex flex-wrap gap-1 mt-1">
              {systemInfo.build_features.map((f) => (
                <span key={f} className="px-2 py-0.5 bg-blue-50 text-blue-700 rounded text-xs font-mono">{f}</span>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
