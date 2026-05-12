import { useState, useEffect, useCallback, useMemo } from 'react';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';
import ModelSelector from '../components/ModelSelector';
import { PRESETS, CLOUD_PROVIDERS, detectPreset } from '../data/presets';
import type { CloudProvider } from '../data/presets';
import { extractProvider } from '../api/modelProviders';

// ── Provider definitions ───────────────────────────────────────────

const ALL_PROVIDERS = [
  { id: 'gemini', label: 'Google Gemini' },
  { id: 'anthropic', label: 'Anthropic' },
  { id: 'openai', label: 'OpenAI' },
  { id: 'ollama', label: 'Ollama (local)' },
  { id: 'deepseek', label: 'DeepSeek' },
  { id: 'groq', label: 'Groq' },
  { id: 'openrouter', label: 'OpenRouter (all models)' },
  { id: 'fireworks', label: 'Fireworks AI' },
  { id: 'together', label: 'Together AI' },
  { id: 'mistral', label: 'Mistral AI' },
  { id: 'perplexity', label: 'Perplexity' },
  { id: 'cerebras', label: 'Cerebras' },
  { id: 'sambanova', label: 'SambaNova' },
  { id: 'xai', label: 'xAI (Grok)' },
  { id: 'elevenlabs', label: 'ElevenLabs' },
  { id: 'replicate', label: 'Replicate' },
  { id: 'assemblyai', label: 'AssemblyAI' },
  { id: 'deepgram', label: 'Deepgram' },
  { id: 'openai-compatible', label: 'OpenAI-Compatible (custom)' },
] as const;

const API_KEY_MAP: Record<string, { label: string; env: string }> = {
  gemini: { label: 'Google Gemini', env: 'GOOGLE_API_KEY' },
  anthropic: { label: 'Anthropic', env: 'ANTHROPIC_API_KEY' },
  openai: { label: 'OpenAI', env: 'OPENAI_API_KEY' },
  deepseek: { label: 'DeepSeek', env: 'DEEPSEEK_API_KEY' },
  groq: { label: 'Groq', env: 'GROQ_API_KEY' },
  together: { label: 'Together', env: 'TOGETHER_API_KEY' },
  fireworks: { label: 'Fireworks', env: 'FIREWORKS_API_KEY' },
  mistral: { label: 'Mistral', env: 'MISTRAL_API_KEY' },
  perplexity: { label: 'Perplexity', env: 'PERPLEXITY_API_KEY' },
  openrouter: { label: 'OpenRouter', env: 'OPENROUTER_API_KEY' },
  cerebras: { label: 'Cerebras', env: 'CEREBRAS_API_KEY' },
  sambanova: { label: 'SambaNova', env: 'SAMBANOVA_API_KEY' },
  xai: { label: 'xAI', env: 'XAI_API_KEY' },
  elevenlabs: { label: 'ElevenLabs', env: 'ELEVENLABS_API_KEY' },
  replicate: { label: 'Replicate', env: 'REPLICATE_API_TOKEN' },
  assemblyai: { label: 'AssemblyAI', env: 'ASSEMBLYAI_API_KEY' },
  deepgram: { label: 'Deepgram', env: 'DEEPGRAM_API_KEY' },
};

// Providers that don't need API keys
const NO_KEY_PROVIDERS = new Set(['ollama', 'openai-compatible']);

// ── Category definitions ───────────────────────────────────────────

interface CategoryDef {
  key: string;
  label: string;
  description: string;
  modality?: string;
  required?: boolean;
  omniFallback?: boolean;
}

const CATEGORIES: CategoryDef[] = [
  { key: 'primary', label: 'Primary (Text)', description: 'Main conversational agent — handles chat, reasoning, and general tasks.', modality: 'text->text', required: true },
  { key: 'vision', label: 'Vision', description: 'Image understanding — analyzes images and screenshots.', modality: 'text+image->text', omniFallback: true },
  { key: 'omni', label: 'Omni', description: 'Multimodal all-in-one — auto-fills Vision, TTS, and STT if those are not set.' },
  { key: 'image_generation', label: 'Image Generation', description: 'Creates images from text descriptions.', modality: 'text->image' },
  { key: 'tts', label: 'TTS (Text-to-Speech)', description: 'Converts text responses to spoken audio.', modality: 'text->audio', omniFallback: true },
  { key: 'stt', label: 'STT (Speech-to-Text)', description: 'Converts spoken audio to text input.', modality: 'audio->text', omniFallback: true },
  { key: 'music', label: 'Music Generation', description: 'Generates music and audio compositions from text prompts.', modality: 'text->music' },
  { key: 'code', label: 'Code', description: 'Specialized for code generation, review, and debugging.', modality: 'text->text' },
  { key: 'embedding', label: 'Embedding', description: 'Generates vector embeddings for RAG and memory search.', modality: 'embedding' },
  { key: 'search', label: 'Search', description: 'Web search grounding — retrieves live information.', modality: 'text->text' },
];

// ── Types ──────────────────────────────────────────────────────────

/** Per-category state: ordered list of model IDs (fallback chain) */
type CategoryModels = Record<string, string[]>;

/** Per-category provider selection for the "add model" row */
type CategoryProviders = Record<string, string>;

// ── Component ──────────────────────────────────────────────────────

export default function AgentModel() {
  const [categories, setCategories] = useState<CategoryModels>({});
  const [categoryProviders, setCategoryProviders] = useState<CategoryProviders>({});
  const [apiKeys, setApiKeys] = useState<Record<string, string>>({});
  const [baseUrl, setBaseUrl] = useState('');
  const [saving, setSaving] = useState(false);
  const [loading, setLoading] = useState(true);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [showSuccessModal, setShowSuccessModal] = useState(false);
  const [selectedPreset, setSelectedPreset] = useState<string | null>(null);
  const [customizedFrom, setCustomizedFrom] = useState<string | null>(null);

  // Enterprise state
  const [cloudProvider, setCloudProvider] = useState<CloudProvider>('azure');
  const [cloudConfig, setCloudConfig] = useState<Record<string, string>>({});
  const [cloudErrors, setCloudErrors] = useState<Record<string, string>>({});

  // Load current config on mount
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await api.getAgent();
        if (cancelled) return;
        if (res.ok && res.data) {
          const d = res.data;
          const cats: CategoryModels = {};
          // primary is always a string from the API
          if (d.primary) cats.primary = [d.primary];
          // Other categories are arrays or null
          for (const key of ['vision', 'omni', 'image_generation', 'tts', 'stt', 'code', 'embedding', 'search', 'music'] as const) {
            const val = d[key];
            if (Array.isArray(val) && val.length > 0) {
              cats[key] = val;
            } else if (typeof val === 'string' && val) {
              // backward compat: single string
              cats[key] = [val];
            }
          }
          setCategories(cats);

          // Detect provider per category from first model
          const provs: CategoryProviders = {};
          for (const [key, models] of Object.entries(cats)) {
            if (models.length > 0) {
              provs[key] = extractProvider(models[0]) || 'openrouter';
            }
          }
          setCategoryProviders(provs);

          // Detect if config matches a preset
          const detected = detectPreset(cats);
          if (detected) setSelectedPreset(detected);

          // Load cloud provider config if present
          if (d.cloud_provider) {
            const cp = d.cloud_provider as Record<string, string>;
            if (cp.type) {
              setCloudProvider(cp.type as CloudProvider);
              const { type: _, ...fields } = cp;
              setCloudConfig(fields);
              setSelectedPreset('enterprise');
            }
          }
        }
      } catch {
        if (!cancelled) {
          setAlert({ type: 'error', message: 'Failed to load current configuration.' });
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // ── Preset handling ────────────────────────────────────────────

  const applyPreset = useCallback((presetId: string) => {
    const preset = PRESETS.find(p => p.id === presetId);
    if (!preset) return;

    setSelectedPreset(presetId);
    setCustomizedFrom(null);

    if (preset.isEnterprise) {
      // Clear all categories for enterprise
      setCategories({});
      setCategoryProviders({});
      return;
    }

    const cats: CategoryModels = {};
    const provs: CategoryProviders = {};
    for (const [key, cat] of Object.entries(preset.categories)) {
      if (cat.models.length > 0) {
        cats[key] = [...cat.models];
        provs[key] = extractProvider(cat.models[0]) || 'openrouter';
      }
    }
    setCategories(cats);
    setCategoryProviders(provs);
  }, []);

  // ── Category model management ──────────────────────────────────

  const addModelToCategory = useCallback((catKey: string, modelId: string) => {
    setCategories(prev => {
      const current = prev[catKey] || [];
      if (current.length >= 5) return prev; // max 5 per category
      if (current.includes(modelId)) return prev;
      const updated = { ...prev, [catKey]: [...current, modelId] };
      // Mark as customized if we had a preset
      return updated;
    });
    if (selectedPreset && !customizedFrom) {
      setCustomizedFrom(selectedPreset);
      setSelectedPreset(null);
    }
  }, [selectedPreset, customizedFrom]);

  const removeModelFromCategory = useCallback((catKey: string, index: number) => {
    setCategories(prev => {
      const current = [...(prev[catKey] || [])];
      current.splice(index, 1);
      if (current.length === 0) {
        const { [catKey]: _, ...rest } = prev;
        return rest;
      }
      return { ...prev, [catKey]: current };
    });
    if (selectedPreset && !customizedFrom) {
      setCustomizedFrom(selectedPreset);
      setSelectedPreset(null);
    }
  }, [selectedPreset, customizedFrom]);

  const moveModel = useCallback((catKey: string, fromIdx: number, toIdx: number) => {
    setCategories(prev => {
      const current = [...(prev[catKey] || [])];
      if (toIdx < 0 || toIdx >= current.length) return prev;
      const [item] = current.splice(fromIdx, 1);
      current.splice(toIdx, 0, item);
      return { ...prev, [catKey]: current };
    });
  }, []);

  const setCategoryProvider = useCallback((catKey: string, providerId: string) => {
    setCategoryProviders(prev => ({ ...prev, [catKey]: providerId }));
  }, []);

  // ── Compute required API keys ──────────────────────────────────

  const requiredProviders = useMemo(() => {
    const providers = new Set<string>();
    for (const models of Object.values(categories)) {
      for (const modelId of models) {
        const prov = extractProvider(modelId);
        if (prov && !NO_KEY_PROVIDERS.has(prov) && API_KEY_MAP[prov]) {
          providers.add(prov);
        }
      }
    }
    return Array.from(providers).sort();
  }, [categories]);

  // ── Save handler ───────────────────────────────────────────────

  const handleSave = async () => {
    const isEnterpriseSave = selectedPreset === 'enterprise';

    // Enterprise validation: check required cloud fields
    if (isEnterpriseSave) {
      const errors: Record<string, string> = {};
      if (cloudProvider === 'azure') {
        if (!cloudConfig.endpoint_url?.trim()) errors.endpoint_url = 'Endpoint URL is required';
        if (!cloudConfig.api_version?.trim()) errors.api_version = 'API Version is required';
        if (!cloudConfig.deployment_name?.trim()) errors.deployment_name = 'Deployment Name is required';
      } else if (cloudProvider === 'gcp') {
        if (!cloudConfig.project_id?.trim()) errors.project_id = 'Project ID is required';
        if (!cloudConfig.region?.trim()) errors.region = 'Region is required';
        if (!cloudConfig.model_name?.trim()) errors.model_name = 'Model Name is required';
      } else if (cloudProvider === 'bedrock') {
        if (!cloudConfig.region?.trim()) errors.region = 'Region is required';
        if (!cloudConfig.aws_profile?.trim()) errors.aws_profile = 'AWS Profile is required';
        if (!cloudConfig.model_arn?.trim()) errors.model_arn = 'Model ARN is required';
      }
      if (Object.keys(errors).length > 0) {
        setCloudErrors(errors);
        setAlert({ type: 'error', message: 'Please fill in all required cloud provider fields.' });
        return;
      }
      setCloudErrors({});
    }

    if (!isEnterpriseSave) {
      const primaryModels = categories.primary;
      if (!primaryModels || primaryModels.length === 0) {
        setAlert({ type: 'error', message: 'Primary model is required.' });
        return;
      }
    }

    setSaving(true);
    setAlert(null);
    try {
      const payload: Record<string, unknown> = {};

      if (isEnterpriseSave) {
        payload.cloud_provider = { type: cloudProvider, ...cloudConfig };
      } else {
        const primaryModels = categories.primary;
        payload.primary = primaryModels!.length === 1 ? primaryModels![0] : primaryModels;
        payload.api_keys = Object.fromEntries(
          Object.entries(apiKeys).filter(([, v]) => v.length > 0),
        );
        if (baseUrl) payload.base_url = baseUrl;

        // Add category fields as arrays
        for (const cat of CATEGORIES) {
          if (cat.key === 'primary') continue;
          const models = categories[cat.key];
          if (models && models.length > 0) {
            payload[cat.key] = models;
          }
        }
      }

      const res = await api.saveAgent(payload);
      if (res.ok) {
        setAlert({ type: 'success', message: res.message || 'Agent configuration saved.' });
        setShowSuccessModal(true);
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  };

  // ── Render ─────────────────────────────────────────────────────

  if (loading) {
    return (
      <div>
        <h2 className="text-2xl font-semibold mb-5">Model Providers</h2>
        <div className="space-y-4 max-w-3xl animate-pulse">
          {[1, 2, 3].map(i => (
            <div key={i} className="bg-white rounded-xl shadow-sm p-6">
              <div className="h-5 bg-gray-200 rounded w-1/3 mb-3" />
              <div className="h-10 bg-gray-100 rounded w-full" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  const isEnterprise = selectedPreset === 'enterprise';

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Model Providers</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      <div className="space-y-6 max-w-3xl">
        {/* ── Preset Selector ─────────────────────────────────── */}
        <div>
          <h3 className="text-lg font-semibold mb-3">🎯 Quick Setup</h3>
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            {PRESETS.map((preset) => (
              <button
                key={preset.id}
                type="button"
                onClick={() => applyPreset(preset.id)}
                className={`text-left p-4 rounded-xl border-2 transition-all ${
                  selectedPreset === preset.id
                    ? 'border-[var(--color-accent)] bg-blue-50 shadow-md'
                    : 'border-gray-200 bg-white hover:border-gray-300 hover:shadow-sm'
                }`}
              >
                <div className="text-2xl mb-1">{preset.icon}</div>
                <div className="font-semibold text-sm">{preset.name}</div>
                <div className="text-xs text-gray-500 mt-1 line-clamp-2">{preset.description}</div>
                <div className="text-xs font-medium text-gray-600 mt-2">{preset.estimatedCost}</div>
              </button>
            ))}
            {/* Custom option */}
            <button
              type="button"
              onClick={() => { setSelectedPreset(null); setCustomizedFrom(null); }}
              className={`text-left p-4 rounded-xl border-2 transition-all ${
                !selectedPreset && !customizedFrom
                  ? 'border-[var(--color-accent)] bg-blue-50 shadow-md'
                  : 'border-gray-200 bg-white hover:border-gray-300 hover:shadow-sm'
              }`}
            >
              <div className="text-2xl mb-1">🔧</div>
              <div className="font-semibold text-sm">Custom</div>
              <div className="text-xs text-gray-500 mt-1">Pick your own models for each category.</div>
            </button>
          </div>
          {customizedFrom && (
            <p className="text-xs text-amber-600 mt-2">
              ✏️ Customized from {PRESETS.find(p => p.id === customizedFrom)?.name || customizedFrom}
            </p>
          )}
        </div>

        {/* ── Enterprise Cloud Config ─────────────────────────── */}
        {isEnterprise && (
          <div className="bg-white rounded-xl shadow-sm p-6">
            <h3 className="text-lg font-semibold mb-3">☁️ Cloud Provider</h3>
            <div className="space-y-3">
              {CLOUD_PROVIDERS.map((cp) => (
                <button
                  key={cp.id}
                  type="button"
                  onClick={() => setCloudProvider(cp.id)}
                  className={`w-full text-left p-3 rounded-lg border-2 transition-all ${
                    cloudProvider === cp.id
                      ? 'border-[var(--color-accent)] bg-blue-50'
                      : 'border-gray-200 hover:border-gray-300'
                  }`}
                >
                  <div className="font-semibold text-sm">{cp.name}</div>
                  <div className="text-xs text-gray-500">{cp.description}</div>
                </button>
              ))}
            </div>

            <div className="mt-4 space-y-3">
              {cloudProvider === 'azure' && (
                <>
                  <ControlledField
                    label="Endpoint URL"
                    placeholder="https://your-resource.openai.azure.com/"
                    value={cloudConfig.endpoint_url || ''}
                    error={cloudErrors.endpoint_url}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, endpoint_url: v }))}
                  />
                  <ControlledField
                    label="API Version"
                    placeholder="2024-02-01"
                    value={cloudConfig.api_version || ''}
                    error={cloudErrors.api_version}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, api_version: v }))}
                  />
                  <ControlledField
                    label="Deployment Name"
                    placeholder="gpt-4o-deployment"
                    value={cloudConfig.deployment_name || ''}
                    error={cloudErrors.deployment_name}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, deployment_name: v }))}
                  />
                </>
              )}
              {cloudProvider === 'gcp' && (
                <>
                  <ControlledField
                    label="Project ID"
                    placeholder="my-gcp-project"
                    value={cloudConfig.project_id || ''}
                    error={cloudErrors.project_id}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, project_id: v }))}
                  />
                  <ControlledField
                    label="Region"
                    placeholder="us-central1"
                    value={cloudConfig.region || ''}
                    error={cloudErrors.region}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, region: v }))}
                  />
                  <ControlledField
                    label="Model Name"
                    placeholder="gemini-2.5-pro"
                    value={cloudConfig.model_name || ''}
                    error={cloudErrors.model_name}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, model_name: v }))}
                  />
                </>
              )}
              {cloudProvider === 'bedrock' && (
                <>
                  <ControlledField
                    label="Region"
                    placeholder="us-east-1"
                    value={cloudConfig.region || ''}
                    error={cloudErrors.region}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, region: v }))}
                  />
                  <ControlledField
                    label="AWS Profile"
                    placeholder="default"
                    value={cloudConfig.aws_profile || ''}
                    error={cloudErrors.aws_profile}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, aws_profile: v }))}
                  />
                  <ControlledField
                    label="Model ARN"
                    placeholder="arn:aws:bedrock:us-east-1::foundation-model/..."
                    value={cloudConfig.model_arn || ''}
                    error={cloudErrors.model_arn}
                    onChange={(v) => setCloudConfig(prev => ({ ...prev, model_arn: v }))}
                  />
                </>
              )}
            </div>
          </div>
        )}

        {/* ── Category Cards ──────────────────────────────────── */}
        {!isEnterprise && (
          <div className="space-y-4">
            <h3 className="text-lg font-semibold">📋 Model Categories</h3>
            {CATEGORIES.map((cat) => {
              const models = categories[cat.key] || [];
              const provider = categoryProviders[cat.key] || 'openrouter';
              const isOmniFilled = cat.omniFallback && models.length === 0 && (categories.omni?.length ?? 0) > 0;

              return (
                <div key={cat.key} className="bg-white rounded-xl shadow-sm p-5">
                  <div className="flex items-center gap-2 mb-1">
                    <h4 className="text-sm font-semibold text-gray-800">
                      {cat.label}
                      {cat.required && <span className="text-red-500 ml-1">*</span>}
                    </h4>
                    {isOmniFilled && (
                      <span className="text-xs bg-purple-50 text-purple-600 px-2 py-0.5 rounded-full">
                        Auto-filled by Omni
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-gray-500 mb-3">{cat.description}</p>

                  {/* Selected models list */}
                  {models.length > 0 && (
                    <div className="space-y-1.5 mb-3">
                      {models.map((modelId, idx) => {
                        const prov = extractProvider(modelId);
                        return (
                          <div key={`${modelId}-${idx}`} className="flex items-center gap-2 bg-gray-50 rounded-lg px-3 py-2">
                            <span className="text-xs font-medium text-gray-400 w-16 shrink-0">
                              {idx === 0 ? 'Primary' : `Fallback ${idx}`}
                            </span>
                            {prov && (
                              <span className="text-xs bg-blue-100 text-blue-700 px-1.5 py-0.5 rounded shrink-0">
                                {prov}
                              </span>
                            )}
                            <span className="text-sm font-mono truncate flex-1">{modelId}</span>
                            <div className="flex items-center gap-1 shrink-0">
                              {idx > 0 && (
                                <button
                                  type="button"
                                  onClick={() => moveModel(cat.key, idx, idx - 1)}
                                  className="text-gray-400 hover:text-gray-600 text-xs px-1"
                                  title="Move up"
                                >↑</button>
                              )}
                              {idx < models.length - 1 && (
                                <button
                                  type="button"
                                  onClick={() => moveModel(cat.key, idx, idx + 1)}
                                  className="text-gray-400 hover:text-gray-600 text-xs px-1"
                                  title="Move down"
                                >↓</button>
                              )}
                              <button
                                type="button"
                                onClick={() => removeModelFromCategory(cat.key, idx)}
                                className="text-red-400 hover:text-red-600 text-xs px-1 ml-1"
                                title="Remove"
                              >✕</button>
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  )}

                  {isOmniFilled && (
                    <p className="text-xs text-purple-500 mb-3">
                      Using Omni model: <span className="font-mono">{categories.omni?.[0]}</span>
                    </p>
                  )}

                  {/* Add model row */}
                  {models.length < 5 && (
                    <div className="flex gap-2 items-end">
                      <div className="w-40 shrink-0">
                        <label className="block text-xs text-gray-500 mb-1">Provider</label>
                        <select
                          value={provider}
                          onChange={(e) => setCategoryProvider(cat.key, e.target.value)}
                          className="w-full px-2 py-2 border border-gray-300 rounded-lg text-xs focus:outline-none focus:border-[var(--color-accent)] bg-white"
                        >
                          {ALL_PROVIDERS.map((p) => (
                            <option key={p.id} value={p.id}>{p.label}</option>
                          ))}
                        </select>
                      </div>
                      <div className="flex-1">
                        <ModelSelector
                          provider={provider}
                          value=""
                          onChange={(v) => addModelToCategory(cat.key, v)}
                          modality={cat.modality}
                        />
                      </div>
                    </div>
                  )}
                  {models.length >= 5 && (
                    <p className="text-xs text-gray-400">Maximum 5 models per category reached.</p>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* ── Dynamic API Keys ────────────────────────────────── */}
        {requiredProviders.length > 0 && (
          <div className="bg-white rounded-xl shadow-sm p-6">
            <h3 className="text-lg font-semibold mb-2">🔑 API Keys</h3>
            <p className="text-xs text-gray-500 mb-4">
              Only showing providers used by your selected models. Keys are stored in the running process.
            </p>
            <div className="space-y-3">
              {requiredProviders.map((provId) => {
                const info = API_KEY_MAP[provId];
                if (!info) return null;
                const hasKey = !!apiKeys[provId];
                return (
                  <div key={provId} className="flex items-center gap-3">
                    <label className="w-28 text-sm text-gray-600 shrink-0 flex items-center gap-1.5">
                      {info.label}
                      {!hasKey && (
                        <span className="text-xs bg-amber-100 text-amber-700 px-1.5 py-0.5 rounded">Required</span>
                      )}
                      {hasKey && (
                        <span className="text-xs bg-green-100 text-green-700 px-1.5 py-0.5 rounded">Set</span>
                      )}
                    </label>
                    <input
                      type="password"
                      value={apiKeys[provId] || ''}
                      onChange={(e) => setApiKeys(prev => ({ ...prev, [provId]: e.target.value }))}
                      placeholder={info.env}
                      className="flex-1 px-3 py-1.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                    />
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {/* ── Base URL for openai-compatible ───────────────────── */}
        {requiredProviders.includes('openai-compatible') && (
          <div className="bg-white rounded-xl shadow-sm p-6">
            <h3 className="text-sm font-semibold mb-2">Custom Endpoint</h3>
            <input
              type="text"
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://your-endpoint.com/v1"
              className="w-full px-3 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>
        )}

        {/* ── Save Button ─────────────────────────────────────── */}
        <button
          onClick={handleSave}
          disabled={saving || (!isEnterprise && !(categories.primary?.length))}
          className="px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
        >
          {saving ? 'Saving...' : 'Save Configuration'}
        </button>
      </div>

      {/* ── Success Modal ─────────────────────────────────────── */}
      {showSuccessModal && (
        <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
          <div className="bg-white rounded-xl shadow-xl max-w-sm w-full p-6 text-center">
            <div className="text-4xl mb-3">✅</div>
            <h3 className="text-lg font-semibold mb-2">Configuration Saved</h3>
            <p className="text-sm text-gray-600 mb-4">
              Primary model: <span className="font-mono font-medium">{categories.primary?.[0]}</span>
              {(categories.primary?.length ?? 0) > 1 && (
                <> with {(categories.primary?.length ?? 1) - 1} fallback(s)</>
              )}
              . The gateway will use these models for new conversations.
            </p>
            <button
              onClick={() => setShowSuccessModal(false)}
              className="px-6 py-2 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)]"
            >
              Done
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Helper Components ──────────────────────────────────────────────

function ControlledField({
  label,
  placeholder,
  value,
  error,
  onChange,
}: {
  label: string;
  placeholder: string;
  value: string;
  error?: string;
  onChange: (value: string) => void;
}) {
  return (
    <div>
      <label className="block text-sm font-medium text-gray-700 mb-1">{label}</label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className={`w-full px-3 py-2 border rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] ${
          error ? 'border-red-300 bg-red-50' : 'border-gray-300'
        }`}
      />
      {error && <p className="text-xs text-red-600 mt-1">{error}</p>}
    </div>
  );
}
