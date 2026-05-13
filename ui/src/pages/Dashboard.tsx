import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import MetricCard from '../components/MetricCard';
import StatusBadge from '../components/StatusBadge';
import type { DashboardData } from '../types';
import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';

function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export default function Dashboard() {
  const { data, loading, error } = useApi<DashboardData>(() => api.dashboard(), []);
  const { lastEvent } = useWebSocket();
  const [liveSessionCount, setLiveSessionCount] = useState<number | null>(null);

  // Update session count from WebSocket events
  useEffect(() => {
    if (lastEvent?.type === 'dashboard') {
      setLiveSessionCount(lastEvent.session_count);
    }
  }, [lastEvent]);

  if (loading) {
    return <div className="text-gray-400">Loading dashboard...</div>;
  }

  if (error || !data) {
    return <div className="text-red-600">Failed to load dashboard: {error}</div>;
  }

  const sessionCount = liveSessionCount ?? data.active_session_count;

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Dashboard</h2>

      {/* Metric cards */}
      <div className="flex flex-wrap gap-4 mb-7">
        <MetricCard label="Uptime" value={formatUptime(data.uptime_secs)} />
        <MetricCard label="Active Sessions" value={sessionCount} />
        <MetricCard label="Channels" value={data.connected_channels.length} />
      </div>

      {/* Pairing Code */}
      <PairingWidget />

      {/* Channels table */}
      <h3 className="text-lg font-semibold mb-3">Channels</h3>
      {data.connected_channels.length === 0 ? (
        <div className="text-center py-12 text-gray-400">No channels connected</div>
      ) : (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Channel</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Account ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Status</th>
              </tr>
            </thead>
            <tbody>
              {data.connected_channels.map((ch, i) => (
                <tr key={i} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-3 text-sm">{ch.channel_type}</td>
                  <td className="px-4 py-3 text-sm font-mono text-gray-600">{ch.account_id}</td>
                  <td className="px-4 py-3"><StatusBadge status={ch.status} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Subsystems */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
        <SubsystemCard title="Memory Service" status={data.memory_status} />
        <SubsystemCard title="RAG Pipeline" status={data.rag_status} />
      </div>

      {/* Onboarding — show setup wizard link or collapsed steps */}
      {data.connected_channels.length === 0 && (
        <SetupOrOnboarding />
      )}
    </div>
  );
}

function SubsystemCard({ title, status }: { title: string; status: DashboardData['memory_status'] }) {
  return (
    <div className="bg-white rounded-xl shadow-sm p-5">
      <h4 className="font-semibold text-sm mb-2">{title}</h4>
      {status ? (
        <div className="text-sm text-gray-600">
          Backend: <strong>{status.backend_type}</strong>
          {' — '}
          <StatusBadge status={status.healthy ? 'healthy' : 'error'} />
          <div className="text-xs text-gray-400 mt-1">{status.details}</div>
        </div>
      ) : (
        <div className="text-sm text-gray-400">Not configured</div>
      )}
    </div>
  );
}

function SetupOrOnboarding() {
  const [expanded, setExpanded] = useState(false);
  const setupComplete = localStorage.getItem('adk_setup_complete') === 'true';

  return (
    <div className="bg-white rounded-xl shadow-sm p-6 border-l-4 border-[var(--color-accent)]">
      {!setupComplete ? (
        <>
          <h3 className="text-lg font-semibold mb-3">🚀 Get Started</h3>
          <p className="text-sm text-gray-600 mb-4">
            Your gateway is running but needs to be configured. Use the setup wizard to get running in under a minute.
          </p>
          <Link
            to="/ui/setup"
            className="inline-flex items-center gap-2 px-5 py-2.5 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] transition-all no-underline"
          >
            ⚡ Complete Setup
          </Link>
        </>
      ) : (
        <>
          <h3 className="text-lg font-semibold mb-3">📡 No Channels Connected</h3>
          <p className="text-sm text-gray-600 mb-4">
            Your model is configured but no channels are active. Connect a channel to start receiving messages.
          </p>
          <div className="flex gap-3 mb-4">
            <Link
              to="/ui/channels"
              className="inline-flex items-center gap-2 px-4 py-2 bg-[var(--color-accent)] text-white rounded-lg font-semibold text-sm hover:bg-[var(--color-accent-hover)] transition-all no-underline"
            >
              Connect Channel
            </Link>
            <Link
              to="/ui/setup"
              className="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-all no-underline"
            >
              Re-run Setup Wizard
            </Link>
          </div>
        </>
      )}

      {/* Collapsed reference steps */}
      <button
        onClick={() => setExpanded(!expanded)}
        className="text-xs text-gray-400 hover:text-gray-600 mt-3 transition-colors"
      >
        {expanded ? '▾ Hide setup steps' : '▸ Show all setup steps'}
      </button>
      {expanded && (
        <div className="mt-3 space-y-2 border-t border-gray-100 pt-3">
          <OnboardingStep num={1} title="Configure your AI model" href="/ui/agent"
            description="Choose a provider (Gemini, Claude, GPT, etc.) and set your API key." />
          <OnboardingStep num={2} title="Connect a channel" href="/ui/channels"
            description="Set up Telegram, Slack, or both. You'll need a bot token." />
          <OnboardingStep num={3} title="Enable memory" href="/ui/settings"
            description="Turn on the knowledge graph so your agent remembers conversations." />
          <OnboardingStep num={4} title="Set up AWP" href="/ui/awp"
            description="Make your gateway discoverable by AI agents via the Agentic Web Protocol." />
          <OnboardingStep num={5} title="Monitor everything" href="/ui/logs"
            description="Watch logs in real-time as messages flow through the gateway." />
        </div>
      )}
    </div>
  );
}

function OnboardingStep({ num, title, description, href }: { num: number; title: string; description: string; href: string }) {
  return (
    <a href={href} className="flex items-start gap-3 p-3 rounded-lg hover:bg-gray-50 transition-colors no-underline">
      <span className="flex-shrink-0 w-7 h-7 rounded-full bg-[var(--color-accent)] text-white text-xs font-bold flex items-center justify-center">
        {num}
      </span>
      <div>
        <div className="text-sm font-semibold text-gray-900">{title}</div>
        <div className="text-xs text-gray-500">{description}</div>
      </div>
    </a>
  );
}

function PairingWidget() {
  const [code, setCode] = useState<string | null>(null);
  const [generating, setGenerating] = useState(false);
  const [status, setStatus] = useState('');

  const generate = useCallback(async () => {
    setGenerating(true);
    setStatus('');
    try {
      const res = await fetch('/pairing/generate', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json' },
      });
      if (res.ok) {
        const json = await res.json();
        setCode(json.code);
        setStatus('Valid for 24 hours');
      } else {
        setStatus('Failed to generate');
      }
    } catch {
      setStatus('Failed to generate');
    } finally {
      setGenerating(false);
    }
  }, []);

  return (
    <div className="bg-white rounded-xl shadow-sm p-5 mb-6 flex items-center gap-4">
      <div className="flex-1">
        <h4 className="text-sm font-semibold text-gray-800 mb-1">🔗 Pairing Code</h4>
        <p className="text-xs text-gray-500">
          Generate a code to pair new users via DM. Send this code to users so they can authenticate with the bot.
        </p>
      </div>
      <div className="flex items-center gap-3 shrink-0">
        {code && (
          <div className="bg-gray-50 border border-gray-200 rounded-lg px-4 py-2 font-mono text-lg tracking-widest text-gray-800">
            {code}
          </div>
        )}
        <div className="text-right">
          <button
            onClick={generate}
            disabled={generating}
            className="px-4 py-2 text-sm font-medium bg-[var(--color-accent)] text-white rounded-lg hover:bg-[var(--color-accent-hover)] disabled:opacity-50"
          >
            {generating ? '...' : code ? 'New Code' : 'Generate'}
          </button>
          {status && <div className="text-xs text-gray-400 mt-1">{status}</div>}
        </div>
      </div>
    </div>
  );
}
