import { useState, useEffect } from 'react';
import { api } from '../api/client';
import AlertBanner from '../components/AlertBanner';

const DM_POLICIES = ['open', 'pairing', 'allowlist', 'disabled'] as const;
const STREAM_MODES = ['partial', 'complete'] as const;

export default function Channels() {
  // Telegram
  const [telegramEnabled, setTelegramEnabled] = useState(false);
  const [telegramToken, setTelegramToken] = useState('');
  const [telegramDmPolicy, setTelegramDmPolicy] = useState<string>('open');
  const [telegramStreamMode, setTelegramStreamMode] = useState<string>('partial');
  const [telegramProbeStatus, setTelegramProbeStatus] = useState<string | null>(null);
  const [telegramProbing, setTelegramProbing] = useState(false);

  // Slack
  const [slackEnabled, setSlackEnabled] = useState(false);
  const [slackBotToken, setSlackBotToken] = useState('');
  const [slackAppToken, setSlackAppToken] = useState('');
  const [slackDmPolicy, setSlackDmPolicy] = useState<string>('open');

  // WhatsApp
  const [whatsappEnabled, setWhatsappEnabled] = useState(false);
  const [whatsappPhoneNumberId, setWhatsappPhoneNumberId] = useState('');
  const [whatsappAccessToken, setWhatsappAccessToken] = useState('');
  const [whatsappVerifyToken, setWhatsappVerifyToken] = useState('');
  const [whatsappWebhookPath, setWhatsappWebhookPath] = useState('/webhook/whatsapp');

  // Discord
  const [discordEnabled, setDiscordEnabled] = useState(false);
  const [discordBotToken, setDiscordBotToken] = useState('');
  const [discordApplicationId, setDiscordApplicationId] = useState('');
  const [discordGuildIds, setDiscordGuildIds] = useState('');

  // Matrix
  const [matrixEnabled, setMatrixEnabled] = useState(false);
  const [matrixHomeserverUrl, setMatrixHomeserverUrl] = useState('');
  const [matrixAccessToken, setMatrixAccessToken] = useState('');
  const [matrixUserId, setMatrixUserId] = useState('');
  const [matrixRoomIds, setMatrixRoomIds] = useState('');

  // Pairing
  const [pairingCode, setPairingCode] = useState<string | null>(null);
  const [pairingStatus, setPairingStatus] = useState<string>('');
  const [generatingCode, setGeneratingCode] = useState(false);

  const [saving, setSaving] = useState(false);
  const [alert, setAlert] = useState<{ type: 'success' | 'error'; message: string } | null>(null);
  const [showNextSteps, setShowNextSteps] = useState(false);
  const [loading, setLoading] = useState(true);

  // Load existing channel config on mount
  useEffect(() => {
    (async () => {
      try {
        const res = await api.getChannels();
        if (res.ok && res.data) {
          const d = res.data;
          if (d.telegram) {
            setTelegramEnabled(d.telegram.enabled);
            setTelegramDmPolicy(d.telegram.dm_policy || 'open');
            setTelegramStreamMode(d.telegram.stream_mode || 'partial');
            // Don't set token — it's masked
          }
          if (d.slack) {
            setSlackEnabled(d.slack.enabled);
            setSlackDmPolicy(d.slack.dm_policy || 'open');
            // Don't set tokens — they're masked
          }
          if (d.whatsapp) {
            setWhatsappEnabled(d.whatsapp.enabled);
            setWhatsappPhoneNumberId(d.whatsapp.phone_number_id || '');
            setWhatsappVerifyToken(d.whatsapp.verify_token || '');
            setWhatsappWebhookPath(d.whatsapp.webhook_path || '/webhook/whatsapp');
            // Don't set access_token — it's masked
          }
          if (d.discord) {
            setDiscordEnabled(d.discord.enabled);
            setDiscordApplicationId(d.discord.application_id || '');
            setDiscordGuildIds((d.discord.guild_ids || []).join(', '));
            // Don't set bot_token — it's masked
          }
          if (d.matrix) {
            setMatrixEnabled(d.matrix.enabled);
            setMatrixHomeserverUrl(d.matrix.homeserver_url || '');
            setMatrixUserId(d.matrix.user_id || '');
            setMatrixRoomIds((d.matrix.room_ids || []).join(', '));
            // Don't set access_token — it's masked
          }
        }
      } catch {
        // Silently fail — form starts with defaults
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleTelegramProbe = async () => {
    setTelegramProbing(true);
    setTelegramProbeStatus(null);
    try {
      const res = await api.probeTelegram();
      if (res.ok && res.data) {
        const data = res.data as { status: string; bot_username?: string };
        if (data.status === 'connected') {
          setTelegramProbeStatus(`✓ Connected (@${data.bot_username})`);
        } else if (data.status === 'invalid_token') {
          setTelegramProbeStatus('✗ Invalid token');
        } else if (data.status === 'unreachable') {
          setTelegramProbeStatus('✗ Unreachable');
        } else {
          setTelegramProbeStatus(`✗ ${data.status}`);
        }
      } else {
        setTelegramProbeStatus(`✗ ${res.message || 'Failed'}`);
      }
    } catch {
      setTelegramProbeStatus('✗ Network error');
    } finally {
      setTelegramProbing(false);
    }
  };

  const handleSave = async () => {
    setSaving(true);
    setAlert(null);
    try {
      const res = await api.saveChannels({
        telegram: {
          enabled: telegramEnabled,
          bot_token: telegramToken,
          dm_policy: telegramDmPolicy,
          stream_mode: telegramStreamMode,
        },
        slack: {
          enabled: slackEnabled,
          bot_token: slackBotToken,
          app_token: slackAppToken,
          dm_policy: slackDmPolicy,
        },
        whatsapp: {
          enabled: whatsappEnabled,
          phone_number_id: whatsappPhoneNumberId,
          access_token: whatsappAccessToken,
          verify_token: whatsappVerifyToken,
          webhook_path: whatsappWebhookPath,
        },
        discord: {
          enabled: discordEnabled,
          bot_token: discordBotToken,
          application_id: discordApplicationId,
          guild_ids: discordGuildIds.split(',').map((s) => s.trim()).filter(Boolean),
        },
        matrix: {
          enabled: matrixEnabled,
          homeserver_url: matrixHomeserverUrl,
          access_token: matrixAccessToken,
          user_id: matrixUserId,
          room_ids: matrixRoomIds.split(',').map((s) => s.trim()).filter(Boolean),
        },
      });
      if (res.ok) {
        setAlert({ type: 'success', message: 'Channel configuration saved.' });
        setShowNextSteps(true);
      } else {
        setAlert({ type: 'error', message: res.message || 'Failed to save.' });
      }
    } catch {
      setAlert({ type: 'error', message: 'Network error.' });
    } finally {
      setSaving(false);
    }
  };

  const generatePairingCode = async () => {
    setGeneratingCode(true);
    try {
      const res = await fetch('/pairing/generate', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
      });
      if (res.ok) {
        const json = await res.json();
        setPairingCode(json.code);
        setPairingStatus(json.status || 'Generated');
      } else {
        setPairingStatus('Failed to generate');
      }
    } catch {
      setPairingStatus('Failed to generate');
    } finally {
      setGeneratingCode(false);
    }
  };

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Channels</h2>

      {alert && (
        <AlertBanner type={alert.type} message={alert.message} onDismiss={() => setAlert(null)} />
      )}

      {loading ? (
        <div className="text-sm text-gray-500">Loading channel configuration...</div>
      ) : (
      <div className="space-y-6 max-w-2xl">
        {/* Pairing */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <h3 className="text-lg font-semibold mb-4">🔗 Pairing</h3>
          <div className="flex items-center gap-3 mb-3">
            <button
              onClick={generatePairingCode}
              disabled={generatingCode}
              className="px-4 py-2 text-sm font-medium bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200 disabled:opacity-50"
            >
              {generatingCode ? 'Generating...' : 'Generate Pairing Code'}
            </button>
            {pairingStatus && (
              <span className="text-sm text-gray-500">{pairingStatus}</span>
            )}
          </div>
          {pairingCode && (
            <div className="bg-gray-50 rounded-lg p-4 font-mono text-lg text-center tracking-widest">
              {pairingCode}
            </div>
          )}
        </div>

        {/* Telegram */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">📱 Telegram</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={telegramEnabled}
                onChange={(e) => setTelegramEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {telegramEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={telegramToken}
                  onChange={(e) => setTelegramToken(e.target.value)}
                  placeholder={telegramEnabled ? "Token configured (leave empty to keep current)" : "Enter Telegram bot token"}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">DM Policy</label>
                <select
                  value={telegramDmPolicy}
                  onChange={(e) => setTelegramDmPolicy(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {DM_POLICIES.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Stream Mode</label>
                <select
                  value={telegramStreamMode}
                  onChange={(e) => setTelegramStreamMode(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {STREAM_MODES.map((m) => <option key={m} value={m}>{m}</option>)}
                </select>
              </div>
              {/* Test Connection */}
              <div className="flex items-center gap-3 pt-1">
                <button
                  type="button"
                  onClick={handleTelegramProbe}
                  disabled={telegramProbing}
                  className="px-3 py-1.5 text-xs font-medium text-blue-700 bg-blue-50 rounded-lg hover:bg-blue-100 disabled:opacity-50"
                >
                  {telegramProbing ? 'Testing...' : 'Test Connection'}
                </button>
                {telegramProbeStatus && (
                  <span className={`text-xs font-medium px-2 py-0.5 rounded-full ${
                    telegramProbeStatus.startsWith('✓')
                      ? 'bg-green-50 text-green-700'
                      : 'bg-red-50 text-red-700'
                  }`}>
                    {telegramProbeStatus}
                  </span>
                )}
              </div>
            </div>
          )}
        </div>

        {/* Slack */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">💬 Slack</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={slackEnabled}
                onChange={(e) => setSlackEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {slackEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={slackBotToken}
                  onChange={(e) => setSlackBotToken(e.target.value)}
                  placeholder="xoxb-..."
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">App Token</label>
                <input
                  type="password"
                  value={slackAppToken}
                  onChange={(e) => setSlackAppToken(e.target.value)}
                  placeholder="xapp-..."
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">DM Policy</label>
                <select
                  value={slackDmPolicy}
                  onChange={(e) => setSlackDmPolicy(e.target.value)}
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                >
                  {DM_POLICIES.map((p) => <option key={p} value={p}>{p}</option>)}
                </select>
              </div>
            </div>
          )}
        </div>

        {/* WhatsApp */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">📲 WhatsApp</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={whatsappEnabled}
                onChange={(e) => setWhatsappEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {whatsappEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Phone Number ID</label>
                <input
                  type="text"
                  value={whatsappPhoneNumberId}
                  onChange={(e) => setWhatsappPhoneNumberId(e.target.value)}
                  placeholder="e.g. 123456789012345"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Access Token</label>
                <input
                  type="password"
                  value={whatsappAccessToken}
                  onChange={(e) => setWhatsappAccessToken(e.target.value)}
                  placeholder="WhatsApp Cloud API access token"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Verify Token</label>
                <input
                  type="text"
                  value={whatsappVerifyToken}
                  onChange={(e) => setWhatsappVerifyToken(e.target.value)}
                  placeholder="Webhook verification token"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Webhook Path</label>
                <input
                  type="text"
                  value={whatsappWebhookPath}
                  onChange={(e) => setWhatsappWebhookPath(e.target.value)}
                  placeholder="/webhook/whatsapp"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
            </div>
          )}
        </div>

        {/* Discord */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">🎮 Discord</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={discordEnabled}
                onChange={(e) => setDiscordEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {discordEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Bot Token</label>
                <input
                  type="password"
                  value={discordBotToken}
                  onChange={(e) => setDiscordBotToken(e.target.value)}
                  placeholder="Discord bot token"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Application ID</label>
                <input
                  type="text"
                  value={discordApplicationId}
                  onChange={(e) => setDiscordApplicationId(e.target.value)}
                  placeholder="e.g. 123456789012345678"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Guild IDs (comma-separated)</label>
                <input
                  type="text"
                  value={discordGuildIds}
                  onChange={(e) => setDiscordGuildIds(e.target.value)}
                  placeholder="e.g. 111222333444, 555666777888"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
            </div>
          )}
        </div>

        {/* Matrix */}
        <div className="bg-white rounded-xl shadow-sm p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-lg font-semibold">🔗 Matrix</h3>
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={matrixEnabled}
                onChange={(e) => setMatrixEnabled(e.target.checked)}
                className="w-4 h-4 rounded border-gray-300 text-[var(--color-accent)] focus:ring-[var(--color-accent)]"
              />
              <span className="text-sm text-gray-600">Enabled</span>
            </label>
          </div>

          {matrixEnabled && (
            <div className="space-y-3">
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Homeserver URL</label>
                <input
                  type="text"
                  value={matrixHomeserverUrl}
                  onChange={(e) => setMatrixHomeserverUrl(e.target.value)}
                  placeholder="e.g. https://matrix.org"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Access Token</label>
                <input
                  type="password"
                  value={matrixAccessToken}
                  onChange={(e) => setMatrixAccessToken(e.target.value)}
                  placeholder="Matrix access token"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">User ID</label>
                <input
                  type="text"
                  value={matrixUserId}
                  onChange={(e) => setMatrixUserId(e.target.value)}
                  placeholder="e.g. @bot:matrix.org"
                  className="w-full px-3 py-2 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)]"
                />
              </div>
              <div>
                <label className="block text-sm font-medium text-gray-700 mb-1">Room IDs (comma-separated)</label>
                <input
                  type="text"
                  value={matrixRoomIds}
                  onChange={(e) => setMatrixRoomIds(e.target.value)}
                  placeholder="e.g. !room1:matrix.org, !room2:matrix.org"
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
          {saving ? 'Saving...' : 'Save Channel Configuration'}
        </button>

        {/* Post-save next steps */}
        {showNextSteps && (
          <div className="bg-blue-50 border border-blue-200 rounded-lg p-4 text-sm text-blue-800">
            <h4 className="font-semibold mb-2">Next Steps</h4>
            <ol className="list-decimal list-inside space-y-1">
              <li>Restart the gateway to apply channel changes</li>
              <li>Verify the bot connects by checking the Dashboard</li>
              <li>Test sending a message through the configured channel</li>
            </ol>
          </div>
        )}
      </div>
      )}
    </div>
  );
}
