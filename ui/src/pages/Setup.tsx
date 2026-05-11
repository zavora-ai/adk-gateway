import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { PRESETS } from '../data/presets';

// ── Types ──────────────────────────────────────────────────────────

type Channel = 'telegram' | 'slack' | 'whatsapp' | 'discord';

interface ChannelCredentials {
  telegram: { bot_token: string };
  slack: { bot_token: string; app_token: string };
  whatsapp: { phone_number_id: string; access_token: string };
  discord: { bot_token: string };
}

// ── Wizard presets — use the real PRESETS from data/presets.ts ──────

// Filter to non-enterprise presets for the wizard cards
const WIZARD_PRESETS = PRESETS.filter(p => !p.isEnterprise);

// Frontier sub-options: which provider to use as primary
const FRONTIER_PROVIDERS = [
  { id: 'openai', label: 'OpenAI', icon: '🟢', model: 'openai/gpt-5.5', env: 'OPENAI_API_KEY', placeholder: 'sk-...' },
  { id: 'anthropic', label: 'Anthropic', icon: '🟠', model: 'anthropic/claude-opus-4.7', env: 'ANTHROPIC_API_KEY', placeholder: 'sk-ant-...' },
  { id: 'google', label: 'Google', icon: '🔵', model: 'google/gemini-3.1-pro-preview', env: 'GOOGLE_API_KEY', placeholder: 'AIza...' },
] as const;

// Auto Intelligence provider options (user picks their primary)
const AUTO_PROVIDERS = [
  { id: 'openai', label: 'OpenAI', icon: '🟢', model: 'openai/gpt-4o', env: 'OPENAI_API_KEY', placeholder: 'sk-...' },
  { id: 'google', label: 'Google', icon: '🔵', model: 'google/gemini-2.5-flash', env: 'GOOGLE_API_KEY', placeholder: 'AIza...' },
  { id: 'anthropic', label: 'Anthropic', icon: '🟠', model: 'anthropic/claude-sonnet-4', env: 'ANTHROPIC_API_KEY', placeholder: 'sk-ant-...' },
] as const;

const CHANNEL_OPTIONS: { id: Channel; icon: string; name: string; desc: string }[] = [
  { id: 'telegram', icon: '📱', name: 'Telegram', desc: 'Most popular — just needs a bot token' },
  { id: 'slack', icon: '💬', name: 'Slack', desc: 'For teams — needs bot + app tokens' },
  { id: 'whatsapp', icon: '📲', name: 'WhatsApp', desc: 'Business API — phone number + token' },
  { id: 'discord', icon: '🎮', name: 'Discord', desc: 'Gaming communities — bot token' },
];

// ── Component ──────────────────────────────────────────────────────

export default function Setup() {
  const navigate = useNavigate();
  const [step, setStep] = useState(1);
  const [direction, setDirection] = useState<'forward' | 'back'>('forward');

  // Step 1 state
  const [selectedPreset, setSelectedPreset] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState('');
  const [apiKeys, setApiKeys] = useState<Record<string, string>>({});
  const [frontierProvider, setFrontierProvider] = useState<string | null>(null);
  const [autoPrimary, setAutoPrimary] = useState<string>('openai');
  const [savingModel, setSavingModel] = useState(false);
  const [modelError, setModelError] = useState('');

  // Step 2 state
  const [selectedChannel, setSelectedChannel] = useState<Channel | null>(null);
  const [channelCreds, setChannelCreds] = useState<ChannelCredentials>({
    telegram: { bot_token: '' },
    slack: { bot_token: '', app_token: '' },
    whatsapp: { phone_number_id: '', access_token: '' },
    discord: { bot_token: '' },
  });
  const [savingChannel, setSavingChannel] = useState(false);
  const [channelError, setChannelError] = useState('');
  const [probeStatus, setProbeStatus] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);

  // Step 3 state
  const [configSummary, setConfigSummary] = useState<{ model: string; channel: string | null }>({
    model: '',
    channel: null,
  });

  // Transition helper
  const goTo = (nextStep: number) => {
    setDirection(nextStep > step ? 'forward' : 'back');
    setStep(nextStep);
  };

  // ── Step 1: Save Model ───────────────────────────────────────────

  const handleSaveModel = async () => {
    if (!selectedPreset) {
      setModelError('Please select a model preset.');
      return;
    }

    // Validate based on preset type
    if (selectedPreset === 'frontier' && !frontierProvider) {
      setModelError('Please select a provider for Frontier.');
      return;
    }

    if (selectedPreset === 'auto' && !apiKeys[autoPrimary]?.trim()) {
      const provLabel = AUTO_PROVIDERS.find(p => p.id === autoPrimary)?.label || autoPrimary;
      setModelError(`${provLabel} API key is required (selected as primary).`);
      return;
    }

    const preset = PRESETS.find(p => p.id === selectedPreset);
    if (!preset) return;

    setSavingModel(true);
    setModelError('');

    try {
      const payload: Record<string, unknown> = {};
      const cats = preset.categories;

      if (selectedPreset === 'frontier' && frontierProvider) {
        // Override primary with the selected frontier provider's model
        const fp = FRONTIER_PROVIDERS.find(p => p.id === frontierProvider);
        if (fp) {
          payload.primary = fp.model;
        }
      } else if (selectedPreset === 'auto') {
        // Build primary chain: selected primary first, then others as fallbacks
        const primary = AUTO_PROVIDERS.find(p => p.id === autoPrimary);
        const fallbacks = AUTO_PROVIDERS.filter(p => p.id !== autoPrimary && apiKeys[p.id]?.trim());
        const models = [primary?.model, ...fallbacks.map(f => f.model)].filter(Boolean) as string[];
        payload.primary = models.length === 1 ? models[0] : models;
      } else if (cats.primary?.models.length) {
        payload.primary = cats.primary.models.length === 1
          ? cats.primary.models[0]
          : cats.primary.models;
      }

      // Add other categories
      for (const [key, cat] of Object.entries(cats)) {
        if (key === 'primary') continue;
        if (cat.models.length > 0) {
          payload[key] = cat.models;
        }
      }

      // Collect API keys based on preset type
      const keysToSave: Record<string, string> = {};

      if (selectedPreset === 'free') {
        if (apiKey.trim()) keysToSave.openrouter = apiKey.trim();
      } else if (selectedPreset === 'frontier') {
        if (apiKey.trim()) {
          const fp = FRONTIER_PROVIDERS.find(p => p.id === frontierProvider);
          if (fp) {
            // Map provider to the correct env key name
            keysToSave[fp.id] = apiKey.trim();
          }
        }
      } else if (selectedPreset === 'auto') {
        for (const [key, val] of Object.entries(apiKeys)) {
          if (val.trim()) keysToSave[key] = val.trim();
        }
      }

      if (Object.keys(keysToSave).length > 0) {
        payload.api_keys = keysToSave;
      }

      const res = await api.saveAgent(payload);
      if (res.ok) {
        const presetName = selectedPreset === 'frontier' && frontierProvider
          ? `Frontier (${FRONTIER_PROVIDERS.find(p => p.id === frontierProvider)?.label})`
          : preset.name;
        setConfigSummary(prev => ({ ...prev, model: presetName }));
        goTo(2);
      } else {
        setModelError(res.message || 'Failed to save model configuration.');
      }
    } catch {
      setModelError('Network error — is the gateway running?');
    } finally {
      setSavingModel(false);
    }
  };

  // ── Step 2: Save Channel ─────────────────────────────────────────

  const handleTestTelegram = async () => {
    setProbing(true);
    setProbeStatus(null);
    try {
      const res = await api.probeTelegram();
      if (res.ok && res.data) {
        const data = res.data;
        if (data.status === 'connected') {
          setProbeStatus(`✓ Connected (@${data.bot_username})`);
        } else if (data.status === 'invalid_token') {
          setProbeStatus('✗ Invalid token');
        } else {
          setProbeStatus(`✗ ${data.status}`);
        }
      } else {
        setProbeStatus(`✗ ${res.message || 'Failed'}`);
      }
    } catch {
      setProbeStatus('✗ Network error');
    } finally {
      setProbing(false);
    }
  };

  const handleSaveChannel = async () => {
    if (!selectedChannel) {
      setChannelError('Please select a channel.');
      return;
    }

    setSavingChannel(true);
    setChannelError('');

    try {
      const channelPayload: Record<string, unknown> = {
        telegram: { enabled: false },
        slack: { enabled: false },
        whatsapp: { enabled: false },
        discord: { enabled: false },
        matrix: { enabled: false },
      };

      if (selectedChannel === 'telegram') {
        channelPayload.telegram = {
          enabled: true,
          bot_token: channelCreds.telegram.bot_token,
          dm_policy: 'open',
          stream_mode: 'partial',
        };
      } else if (selectedChannel === 'slack') {
        channelPayload.slack = {
          enabled: true,
          bot_token: channelCreds.slack.bot_token,
          app_token: channelCreds.slack.app_token,
          dm_policy: 'open',
        };
      } else if (selectedChannel === 'whatsapp') {
        channelPayload.whatsapp = {
          enabled: true,
          phone_number_id: channelCreds.whatsapp.phone_number_id,
          access_token: channelCreds.whatsapp.access_token,
          verify_token: '',
          webhook_path: '/webhook/whatsapp',
        };
      } else if (selectedChannel === 'discord') {
        channelPayload.discord = {
          enabled: true,
          bot_token: channelCreds.discord.bot_token,
          application_id: '',
          guild_ids: [],
        };
      }

      const res = await api.saveChannels(channelPayload);
      if (res.ok) {
        const ch = CHANNEL_OPTIONS.find(c => c.id === selectedChannel);
        setConfigSummary(prev => ({ ...prev, channel: ch?.name || selectedChannel }));
        finishSetup();
        goTo(3);
      } else {
        setChannelError(res.message || 'Failed to save channel.');
      }
    } catch {
      setChannelError('Network error.');
    } finally {
      setSavingChannel(false);
    }
  };

  const handleSkipChannel = () => {
    setConfigSummary(prev => ({ ...prev, channel: null }));
    finishSetup();
    goTo(3);
  };

  // ── Finish ───────────────────────────────────────────────────────

  const finishSetup = () => {
    localStorage.setItem('adk_setup_complete', 'true');
  };

  const handleGoToDashboard = () => {
    navigate('/ui');
  };

  // ── Render ───────────────────────────────────────────────────────

  return (
    <div className="min-h-screen bg-gray-50 flex flex-col">
      {/* Progress indicator */}
      <div className="bg-white border-b border-gray-200 px-6 py-4">
        <div className="max-w-2xl mx-auto">
          <div className="flex items-center justify-between mb-2">
            <h1 className="text-lg font-semibold text-gray-900">
              <span className="text-[var(--color-accent)]">⚡</span> Setup Wizard
            </h1>
            <span className="text-sm text-gray-500">Step {step} of 3</span>
          </div>
          <div className="flex gap-2">
            {[1, 2, 3].map(s => (
              <div
                key={s}
                className={`h-1.5 flex-1 rounded-full transition-all duration-300 ${
                  s <= step ? 'bg-[var(--color-accent)]' : 'bg-gray-200'
                }`}
              />
            ))}
          </div>
          <div className="flex justify-between mt-2">
            <StepLabel num={1} label="Choose Model" active={step >= 1} />
            <StepLabel num={2} label="Connect Channel" active={step >= 2} />
            <StepLabel num={3} label="Ready!" active={step >= 3} />
          </div>
        </div>
      </div>

      {/* Step content */}
      <div className="flex-1 flex items-start justify-center px-4 py-8">
        <div className={`w-full max-w-2xl transition-all duration-300 ${
          direction === 'forward' ? 'animate-fade-in-right' : 'animate-fade-in-left'
        }`} key={step}>
          {step === 1 && (
            <StepChooseModel
              selectedPreset={selectedPreset}
              onSelectPreset={setSelectedPreset}
              apiKey={apiKey}
              onApiKeyChange={setApiKey}
              apiKeys={apiKeys}
              onApiKeysChange={setApiKeys}
              frontierProvider={frontierProvider}
              onFrontierProviderChange={setFrontierProvider}
              autoPrimary={autoPrimary}
              onAutoPrimaryChange={setAutoPrimary}
              onNext={handleSaveModel}
              saving={savingModel}
              error={modelError}
            />
          )}
          {step === 2 && (
            <StepConnectChannel
              selectedChannel={selectedChannel}
              onSelectChannel={setSelectedChannel}
              channelCreds={channelCreds}
              onCredsChange={setChannelCreds}
              onNext={handleSaveChannel}
              onSkip={handleSkipChannel}
              onBack={() => goTo(1)}
              saving={savingChannel}
              error={channelError}
              probeStatus={probeStatus}
              probing={probing}
              onTestTelegram={handleTestTelegram}
            />
          )}
          {step === 3 && (
            <StepComplete
              summary={configSummary}
              onDashboard={handleGoToDashboard}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// ── Step Label ─────────────────────────────────────────────────────

function StepLabel({ num, label, active }: { num: number; label: string; active: boolean }) {
  return (
    <span className={`text-xs font-medium ${active ? 'text-[var(--color-accent)]' : 'text-gray-400'}`}>
      {num}. {label}
    </span>
  );
}

// ── Step 1: Choose Model ───────────────────────────────────────────

function StepChooseModel({
  selectedPreset,
  onSelectPreset,
  apiKey,
  onApiKeyChange,
  apiKeys,
  onApiKeysChange,
  frontierProvider,
  onFrontierProviderChange,
  autoPrimary,
  onAutoPrimaryChange,
  onNext,
  saving,
  error,
}: {
  selectedPreset: string | null;
  onSelectPreset: (id: string) => void;
  apiKey: string;
  onApiKeyChange: (v: string) => void;
  apiKeys: Record<string, string>;
  onApiKeysChange: (keys: Record<string, string>) => void;
  frontierProvider: string | null;
  onFrontierProviderChange: (id: string) => void;
  autoPrimary: string;
  onAutoPrimaryChange: (id: string) => void;
  onNext: () => void;
  saving: boolean;
  error: string;
}) {
  return (
    <div>
      <div className="text-center mb-8">
        <h2 className="text-2xl font-bold text-gray-900 mb-2">Choose Your AI Model</h2>
        <p className="text-gray-500">Pick a preset to get started quickly. You can customize later.</p>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mb-6">
        {WIZARD_PRESETS.map(preset => (
          <button
            key={preset.id}
            type="button"
            onClick={() => onSelectPreset(preset.id)}
            className={`text-left p-5 rounded-xl border-2 transition-all ${
              selectedPreset === preset.id
                ? 'border-[var(--color-accent)] bg-blue-50 shadow-md scale-[1.02]'
                : 'border-gray-200 bg-white hover:border-gray-300 hover:shadow-sm'
            }`}
          >
            <div className="text-3xl mb-2">{preset.icon}</div>
            <div className="font-semibold text-gray-900">{preset.name}</div>
            <div className="text-xs text-gray-500 mt-1">{preset.description}</div>
            <div className="text-xs font-medium text-[var(--color-accent)] mt-2">{preset.estimatedCost}</div>
          </button>
        ))}
      </div>

      {/* Free Tier: single OpenRouter key (optional) */}
      {selectedPreset === 'free' && (
        <div className="bg-white rounded-xl border border-gray-200 p-5 mb-6">
          <label className="block text-sm font-medium text-gray-700 mb-2">
            🔑 OpenRouter API Key <span className="text-gray-400 font-normal">(optional)</span>
          </label>
          <input
            type="password"
            value={apiKey}
            onChange={(e) => onApiKeyChange(e.target.value)}
            placeholder="sk-or-..."
            className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
          />
          <p className="text-xs text-gray-400 mt-2">
            Free tier works without a key, but adding one gives higher rate limits. Get one at{' '}
            <a href="https://openrouter.ai/keys" target="_blank" rel="noopener noreferrer" className="text-[var(--color-accent)] hover:underline">openrouter.ai</a>
          </p>
        </div>
      )}

      {/* Frontier: choose provider (OpenAI / Anthropic / Google) */}
      {selectedPreset === 'frontier' && (
        <div className="bg-white rounded-xl border border-gray-200 p-5 mb-6">
          <label className="block text-sm font-medium text-gray-700 mb-3">
            Choose your frontier provider
          </label>
          <div className="grid grid-cols-3 gap-3 mb-4">
            {FRONTIER_PROVIDERS.map(fp => (
              <button
                key={fp.id}
                type="button"
                onClick={() => onFrontierProviderChange(fp.id)}
                className={`text-center p-3 rounded-lg border-2 transition-all ${
                  frontierProvider === fp.id
                    ? 'border-[var(--color-accent)] bg-blue-50'
                    : 'border-gray-200 hover:border-gray-300'
                }`}
              >
                <div className="text-xl mb-1">{fp.icon}</div>
                <div className="text-sm font-medium">{fp.label}</div>
              </button>
            ))}
          </div>
          {frontierProvider && (
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">
                🔑 {FRONTIER_PROVIDERS.find(p => p.id === frontierProvider)?.label} API Key
              </label>
              <input
                type="password"
                value={apiKey}
                onChange={(e) => onApiKeyChange(e.target.value)}
                placeholder={FRONTIER_PROVIDERS.find(p => p.id === frontierProvider)?.placeholder}
                className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)]"
              />
            </div>
          )}
        </div>
      )}

      {/* Auto Intelligence: choose primary + multiple API keys */}
      {selectedPreset === 'auto' && (
        <div className="bg-white rounded-xl border border-gray-200 p-5 mb-6">
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Choose your primary provider
          </label>
          <p className="text-xs text-gray-400 mb-3">
            Your primary handles most requests. Others are used as fallbacks when the primary fails.
          </p>

          {/* Primary selector */}
          <div className="grid grid-cols-3 gap-3 mb-5">
            {AUTO_PROVIDERS.map(prov => (
              <button
                key={prov.id}
                type="button"
                onClick={() => onAutoPrimaryChange(prov.id)}
                className={`text-center p-3 rounded-lg border-2 transition-all ${
                  autoPrimary === prov.id
                    ? 'border-[var(--color-accent)] bg-blue-50'
                    : 'border-gray-200 hover:border-gray-300'
                }`}
              >
                <div className="text-xl mb-1">{prov.icon}</div>
                <div className="text-sm font-medium">{prov.label}</div>
                {autoPrimary === prov.id && (
                  <div className="text-[10px] text-[var(--color-accent)] font-semibold mt-1">PRIMARY</div>
                )}
              </button>
            ))}
          </div>

          {/* API key inputs */}
          <div className="space-y-3">
            {AUTO_PROVIDERS.map(prov => {
              const isPrimary = autoPrimary === prov.id;
              return (
                <div key={prov.id}>
                  <label className="block text-xs font-medium text-gray-600 mb-1">
                    {prov.icon} {prov.label}
                    {isPrimary
                      ? <span className="text-red-500 ml-1">* primary</span>
                      : <span className="text-gray-400 ml-1">(fallback)</span>
                    }
                  </label>
                  <input
                    type="password"
                    value={apiKeys[prov.id] || ''}
                    onChange={(e) => onApiKeysChange({ ...apiKeys, [prov.id]: e.target.value })}
                    placeholder={prov.placeholder}
                    className={`w-full px-4 py-2.5 border rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] focus:ring-1 focus:ring-[var(--color-accent)] ${
                      isPrimary ? 'border-[var(--color-accent)] bg-blue-50/30' : 'border-gray-300'
                    }`}
                  />
                </div>
              );
            })}
          </div>
          <p className="text-xs text-gray-400 mt-3">
            Only the primary key is required. Add fallback keys for automatic retry on failures.
          </p>
        </div>
      )}

      {error && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-3 mb-4 text-sm text-red-700">
          {error}
        </div>
      )}

      <div className="flex justify-end">
        <button
          onClick={onNext}
          disabled={!selectedPreset || saving}
          className="px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-all"
        >
          {saving ? 'Saving...' : 'Next →'}
        </button>
      </div>
    </div>
  );
}

// ── Step 2: Connect Channel ────────────────────────────────────────

function StepConnectChannel({
  selectedChannel,
  onSelectChannel,
  channelCreds,
  onCredsChange,
  onNext,
  onSkip,
  onBack,
  saving,
  error,
  probeStatus,
  probing,
  onTestTelegram,
}: {
  selectedChannel: Channel | null;
  onSelectChannel: (ch: Channel) => void;
  channelCreds: ChannelCredentials;
  onCredsChange: (creds: ChannelCredentials) => void;
  onNext: () => void;
  onSkip: () => void;
  onBack: () => void;
  saving: boolean;
  error: string;
  probeStatus: string | null;
  probing: boolean;
  onTestTelegram: () => void;
}) {
  const updateCred = (channel: Channel, field: string, value: string) => {
    onCredsChange({
      ...channelCreds,
      [channel]: { ...channelCreds[channel], [field]: value },
    });
  };

  return (
    <div>
      <div className="text-center mb-8">
        <h2 className="text-2xl font-bold text-gray-900 mb-2">Connect a Channel</h2>
        <p className="text-gray-500">Where should your AI agent receive messages?</p>
      </div>

      {/* Channel selector */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-6">
        {CHANNEL_OPTIONS.map(ch => (
          <button
            key={ch.id}
            type="button"
            onClick={() => onSelectChannel(ch.id)}
            className={`text-center p-4 rounded-xl border-2 transition-all ${
              selectedChannel === ch.id
                ? 'border-[var(--color-accent)] bg-blue-50 shadow-md'
                : 'border-gray-200 bg-white hover:border-gray-300 hover:shadow-sm'
            }`}
          >
            <div className="text-2xl mb-1">{ch.icon}</div>
            <div className="font-semibold text-sm">{ch.name}</div>
          </button>
        ))}
      </div>

      {/* Channel credentials */}
      {selectedChannel && (
        <div className="bg-white rounded-xl border border-gray-200 p-5 mb-6">
          {selectedChannel === 'telegram' && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={channelCreds.telegram.bot_token}
                  onChange={(e) => updateCred('telegram', 'bot_token', e.target.value)}
                  placeholder="123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
                <p className="text-xs text-gray-400 mt-1">Get this from @BotFather on Telegram</p>
              </div>
              <div className="flex items-center gap-3">
                <button
                  type="button"
                  onClick={onTestTelegram}
                  disabled={probing || !channelCreds.telegram.bot_token}
                  className="px-3 py-1.5 text-xs font-medium text-blue-700 bg-blue-50 rounded-lg hover:bg-blue-100 disabled:opacity-50"
                >
                  {probing ? 'Testing...' : 'Test Connection'}
                </button>
                {probeStatus && (
                  <span className={`text-xs font-medium px-2 py-0.5 rounded-full ${
                    probeStatus.startsWith('✓')
                      ? 'bg-green-50 text-green-700'
                      : 'bg-red-50 text-red-700'
                  }`}>
                    {probeStatus}
                  </span>
                )}
              </div>
            </div>
          )}

          {selectedChannel === 'slack' && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={channelCreds.slack.bot_token}
                  onChange={(e) => updateCred('slack', 'bot_token', e.target.value)}
                  placeholder="xoxb-..."
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">App Token</label>
                <input
                  type="password"
                  value={channelCreds.slack.app_token}
                  onChange={(e) => updateCred('slack', 'app_token', e.target.value)}
                  placeholder="xapp-..."
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
                <p className="text-xs text-gray-400 mt-1">Both tokens from your Slack App settings</p>
              </div>
            </div>
          )}

          {selectedChannel === 'whatsapp' && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Phone Number ID</label>
                <input
                  type="text"
                  value={channelCreds.whatsapp.phone_number_id}
                  onChange={(e) => updateCred('whatsapp', 'phone_number_id', e.target.value)}
                  placeholder="123456789012345"
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Access Token</label>
                <input
                  type="password"
                  value={channelCreds.whatsapp.access_token}
                  onChange={(e) => updateCred('whatsapp', 'access_token', e.target.value)}
                  placeholder="WhatsApp Cloud API access token"
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
                <p className="text-xs text-gray-400 mt-1">From Meta Business Suite → WhatsApp</p>
              </div>
            </div>
          )}

          {selectedChannel === 'discord' && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={channelCreds.discord.bot_token}
                  onChange={(e) => updateCred('discord', 'bot_token', e.target.value)}
                  placeholder="Discord bot token"
                  className="w-full px-4 py-2.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
                <p className="text-xs text-gray-400 mt-1">From Discord Developer Portal → Bot</p>
              </div>
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-3 mb-4 text-sm text-red-700">
          {error}
        </div>
      )}

      <div className="flex justify-between">
        <button
          onClick={onBack}
          className="px-4 py-2.5 text-sm font-medium text-gray-600 hover:text-gray-900 transition-colors"
        >
          ← Back
        </button>
        <div className="flex gap-3">
          <button
            onClick={onSkip}
            className="px-4 py-2.5 text-sm font-medium text-gray-500 hover:text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-all"
          >
            Skip for now
          </button>
          <button
            onClick={onNext}
            disabled={!selectedChannel || saving}
            className="px-6 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-all"
          >
            {saving ? 'Saving...' : 'Next →'}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Step 3: Complete ───────────────────────────────────────────────

function StepComplete({
  summary,
  onDashboard,
}: {
  summary: { model: string; channel: string | null };
  onDashboard: () => void;
}) {
  const [showConfetti, setShowConfetti] = useState(true);

  useEffect(() => {
    const timer = setTimeout(() => setShowConfetti(false), 3000);
    return () => clearTimeout(timer);
  }, []);

  return (
    <div className="text-center">
      {/* Confetti animation */}
      {showConfetti && (
        <div className="fixed inset-0 pointer-events-none overflow-hidden z-50">
          {Array.from({ length: 50 }).map((_, i) => (
            <div
              key={i}
              className="absolute animate-confetti"
              style={{
                left: `${Math.random() * 100}%`,
                animationDelay: `${Math.random() * 2}s`,
                animationDuration: `${2 + Math.random() * 2}s`,
              }}
            >
              <div
                className="w-2 h-2 rounded-sm"
                style={{
                  backgroundColor: ['#3b82f6', '#10b981', '#f59e0b', '#ef4444', '#8b5cf6', '#ec4899'][
                    Math.floor(Math.random() * 6)
                  ],
                  transform: `rotate(${Math.random() * 360}deg)`,
                }}
              />
            </div>
          ))}
        </div>
      )}

      <div className="text-6xl mb-4">🎉</div>
      <h2 className="text-2xl font-bold text-gray-900 mb-2">You're All Set!</h2>
      <p className="text-gray-500 mb-8">Your AI gateway is configured and ready to go.</p>

      {/* Summary */}
      <div className="bg-white rounded-xl border border-gray-200 p-6 mb-8 text-left max-w-md mx-auto">
        <h3 className="text-sm font-semibold text-gray-700 mb-3">Configuration Summary</h3>
        <div className="space-y-2">
          <div className="flex items-center gap-3">
            <span className="text-green-500">✓</span>
            <span className="text-sm text-gray-700">
              Model: <strong>{summary.model}</strong>
            </span>
          </div>
          <div className="flex items-center gap-3">
            <span className={summary.channel ? 'text-green-500' : 'text-gray-300'}>
              {summary.channel ? '✓' : '○'}
            </span>
            <span className="text-sm text-gray-700">
              {summary.channel
                ? <>Channel: <strong>{summary.channel}</strong></>
                : <span className="text-gray-400">No channel connected (set up later)</span>
              }
            </span>
          </div>
        </div>
      </div>

      {/* Actions */}
      <button
        onClick={onDashboard}
        className="px-8 py-3 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] transition-all shadow-md mb-6"
      >
        Go to Dashboard →
      </button>

      {/* Quick links */}
      <div className="flex flex-wrap justify-center gap-3">
        <QuickLink href="/ui/memory" icon="🧠" label="Add Memory" />
        <QuickLink href="/ui/agents" icon="🧩" label="Create Agents" />
        <QuickLink href="/ui/logs" icon="📋" label="View Logs" />
        <QuickLink href="/ui/channels" icon="📡" label="Manage Channels" />
      </div>
    </div>
  );
}

function QuickLink({ href, icon, label }: { href: string; icon: string; label: string }) {
  return (
    <a
      href={href}
      className="inline-flex items-center gap-1.5 px-3 py-2 text-xs font-medium text-gray-600 bg-white border border-gray-200 rounded-lg hover:border-gray-300 hover:shadow-sm transition-all no-underline"
    >
      <span>{icon}</span>
      {label}
    </a>
  );
}
