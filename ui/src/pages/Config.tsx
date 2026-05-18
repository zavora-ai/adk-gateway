import { useApi } from '../hooks/useApi';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';
import { useState } from 'react';

/** Mask encrypted values in config display */
function maskEncryptedValues(obj: unknown): unknown {
  if (typeof obj === 'string') {
    if (obj.startsWith('enc:')) return 'enc:****';
    return obj;
  }
  if (Array.isArray(obj)) return obj.map(maskEncryptedValues);
  if (obj && typeof obj === 'object') {
    const result: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(obj as Record<string, unknown>)) {
      result[key] = maskEncryptedValues(value);
    }
    return result;
  }
  return obj;
}

export default function Config() {
  const { data, loading, error, refetch } = useApi<Record<string, unknown>>(() => api.config(), []);
  const [editing, setEditing] = useState(false);
  const [editContent, setEditContent] = useState('');
  const [saving, setSaving] = useState(false);
  const [jsonError, setJsonError] = useState<string | null>(null);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);

  const startEdit = () => {
    setEditContent(JSON.stringify(data, null, 2));
    setJsonError(null);
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setJsonError(null);
  };

  const handleSave = async () => {
    // Validate JSON
    try {
      JSON.parse(editContent);
    } catch (e) {
      setJsonError(e instanceof Error ? e.message : 'Invalid JSON');
      return;
    }

    setSaving(true);
    setAlert(null);
    try {
      const res = await api.saveConfig(editContent);
      if (res.ok) {
        setAlert({ type: 'success', message: 'Configuration saved successfully.' });
        setEditing(false);
        refetch();
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  };

  if (loading) return <div className="text-gray-400">Loading configuration...</div>;
  if (error) return <div className="text-red-600">Failed to load config: {error}</div>;

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Configuration</h2>

      {alert && (
        <AlertBanner
          type={alert.type}
          message={alert.message}
          onDismiss={() => setAlert(null)}
        />
      )}

      <div className="bg-white rounded-xl shadow-sm p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="text-sm text-gray-500">
            Redacted configuration (sensitive values hidden)
          </div>
          {!editing ? (
            <button
              onClick={startEdit}
              className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)]"
            >
              Edit
            </button>
          ) : (
            <div className="flex gap-2">
              <button
                onClick={cancelEdit}
                className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200"
              >
                Cancel
              </button>
              <button
                onClick={handleSave}
                disabled={saving}
                className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
              >
                {saving ? 'Saving...' : 'Save'}
              </button>
            </div>
          )}
        </div>

        {jsonError && (
          <div className="bg-red-50 border border-red-200 text-red-800 rounded-lg px-4 py-3 mb-4 text-sm">
            Invalid JSON: {jsonError}
          </div>
        )}

        {editing ? (
          <textarea
            value={editContent}
            onChange={(e) => { setEditContent(e.target.value); setJsonError(null); }}
            className="w-full h-[500px] font-mono text-sm p-4 border border-gray-300 rounded-lg focus:outline-none focus:border-[var(--color-accent)] bg-gray-50"
            spellCheck={false}
          />
        ) : (
          <pre className="bg-gray-900 text-green-400 rounded-lg p-4 overflow-auto text-sm font-mono max-h-[600px]">
            {data ? JSON.stringify(maskEncryptedValues(data), null, 2) : 'No configuration loaded'}
          </pre>
        )}
      </div>
    </div>
  );
}
