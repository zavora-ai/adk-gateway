import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import MetricCard from '../components/MetricCard';
import StatusBadge from '../components/StatusBadge';
import ApprovalBadge from '../components/ApprovalBadge';
import HealthTimeline from '../components/HealthTimeline';
import type { DashboardData, HealthComponent, HealthEvent, PairedUser, UserActivity, PendingApproval, SystemInfo, RestartStatus, LogStorageMetrics, RateLimitMetrics } from '../types';
import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import ConfirmDialog from '../components/ConfirmDialog';

function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const val = bytes / Math.pow(1024, i);
  return `${val.toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
}

export default function Dashboard() {
  const { data, loading, error } = useApi<DashboardData>(() => api.dashboard(), []);
  const { lastEvent } = useWebSocket();
  const [liveSessionCount, setLiveSessionCount] = useState<number | null>(null);

  // Phase 2 data
  const { data: pendingApprovals } = useApi<PendingApproval[]>(() => api.pendingApprovals(), []);
  const { data: healthComponents } = useApi<HealthComponent[]>(() => api.getHealthComponents(), []);
  const { data: healthEvents } = useApi<HealthEvent[]>(() => api.getHealthEvents(), []);
  const { data: pairedUsers } = useApi<PairedUser[]>(() => api.getPairedUsers(), []);
  const { data: userActivities } = useApi<UserActivity[]>(() => api.getUserActivities(), []);
  const { data: systemInfo } = useApi<SystemInfo>(() => api.getSystemInfo(), []);
  const { data: restartStatus } = useApi<RestartStatus>(() => api.getRestartStatus(), []);
  const { data: logStorage } = useApi<LogStorageMetrics>(() => api.getLogStorageMetrics(), []);
  const { data: rateLimitMetrics } = useApi<RateLimitMetrics>(() => api.getRateLimitMetrics(), []);

  const [selectedHealthComponent, setSelectedHealthComponent] = useState<HealthComponent | null>(null);
  const [confirmRestart, setConfirmRestart] = useState(false);
  const [restarting, setRestarting] = useState(false);

  // Update session count from WebSocket events
  useEffect(() => {
    if (lastEvent?.type === 'dashboard') {
      setLiveSessionCount(lastEvent.session_count);
    }
  }, [lastEvent]);

  const handleRestart = async () => {
    setRestarting(true);
    try {
      await api.triggerRestart();
    } catch {
      // Error handled silently
    }
    setConfirmRestart(false);

    // Poll health endpoint and reload once the server is back
    const pollHealth = async () => {
      for (let i = 0; i < 30; i++) {
        await new Promise((r) => setTimeout(r, 2000));
        try {
          const res = await fetch('/health');
          if (res.ok) {
            window.location.reload();
            return;
          }
        } catch {
          // Server still down, keep polling
        }
      }
      // Give up after 60s — user can manually refresh
      setRestarting(false);
    };
    pollHealth();
  };

  if (loading) {
    return <div className="text-gray-400">Loading dashboard...</div>;
  }

  if (error || !data) {
    return <div className="text-red-600">Failed to load dashboard: {error}</div>;
  }

  const sessionCount = liveSessionCount ?? data.active_session_count;
  const pendingCount = pendingApprovals?.length ?? 0;

  // Group paired users by channel
  const usersByChannel: Record<string, number> = {};
  if (pairedUsers) {
    for (const u of pairedUsers) {
      usersByChannel[u.channel] = (usersByChannel[u.channel] || 0) + 1;
    }
  }

  return (
    <div>
      <h2 className="text-2xl font-semibold mb-5">Dashboard</h2>

      {/* Confirm restart dialog */}
      {confirmRestart && (
        <ConfirmDialog
          title="Restart Gateway"
          message="This will trigger a graceful restart. In-flight requests will be drained before shutdown. Are you sure?"
          confirmLabel="Restart"
          onConfirm={handleRestart}
          onCancel={() => setConfirmRestart(false)}
          destructive
        />
      )}

      {/* Health Timeline Detail */}
      {selectedHealthComponent && healthEvents && (
        <HealthTimeline
          component={selectedHealthComponent}
          events={healthEvents}
          onClose={() => setSelectedHealthComponent(null)}
        />
      )}

      {/* Metric cards */}
      <div className="flex flex-wrap gap-4 mb-7">
        <MetricCard label="Uptime" value={formatUptime(data.uptime_secs)} />
        <MetricCard label="Active Sessions" value={sessionCount} />
        <MetricCard label="Channels" value={data.connected_channels.length} />
        {pendingCount > 0 && (
          <MetricCard label="Pending Approvals" value={pendingCount} color="text-orange-600" linkTo="/ui/logs" />
        )}
        <MetricCard label="Paired Users" value={pairedUsers?.length ?? 0} />
        {rateLimitMetrics && (
          <MetricCard label="Rate Limits Today" value={rateLimitMetrics.triggered_today} color={rateLimitMetrics.triggered_today > 0 ? 'text-orange-600' : undefined} />
        )}
        {logStorage && (
          <MetricCard label="Log Files" value={logStorage.file_count} />
        )}
      </div>

      {/* Health Status Row (Task 18) */}
      {healthComponents && healthComponents.length > 0 && (
        <div className="mb-6">
          <h3 className="text-lg font-semibold mb-3">System Health</h3>
          <div className="flex flex-wrap gap-3">
            {healthComponents.map((comp) => {
              const color =
                comp.status === 'healthy'
                  ? 'border-green-300 bg-green-50'
                  : comp.status === 'degraded'
                    ? 'border-yellow-300 bg-yellow-50'
                    : 'border-red-300 bg-red-50';
              const dotColor =
                comp.status === 'healthy'
                  ? 'bg-green-500'
                  : comp.status === 'degraded'
                    ? 'bg-yellow-500'
                    : 'bg-red-500';
              return (
                <button
                  key={comp.name}
                  onClick={() => setSelectedHealthComponent(comp)}
                  className={`flex items-center gap-2 px-4 py-2.5 rounded-lg border ${color} hover:shadow-sm transition-shadow cursor-pointer`}
                >
                  <span className={`inline-block w-2.5 h-2.5 rounded-full ${dotColor}`} />
                  <span className="text-sm font-medium text-gray-800">{comp.name}</span>
                  {comp.consecutive_failures > 0 && (
                    <span className="text-xs text-red-600 font-mono">({comp.consecutive_failures})</span>
                  )}
                </button>
              );
            })}
          </div>
        </div>
      )}

      {/* Pending Approvals (Task 14) */}
      {pendingApprovals && pendingApprovals.length > 0 && (
        <div className="bg-white rounded-xl shadow-sm p-5 mb-6">
          <div className="flex items-center gap-2 mb-3">
            <h3 className="text-lg font-semibold">Approvals</h3>
            <ApprovalBadge count={pendingCount} />
          </div>
          <div className="space-y-2">
            {pendingApprovals.slice(0, 5).map((approval) => (
              <div key={approval.id} className="flex items-center justify-between p-3 bg-orange-50 border border-orange-200 rounded-lg">
                <div>
                  <span className="text-sm font-medium text-gray-800">{approval.tool_name}</span>
                  <span className="text-xs text-gray-500 ml-2">⏳ Waiting for approval...</span>
                </div>
                <div className="flex gap-2">
                  <button
                    onClick={() => api.approveRequest(approval.id)}
                    className="px-3 py-1 text-xs font-medium text-green-700 bg-green-50 rounded-lg hover:bg-green-100"
                  >
                    ✅ Approve
                  </button>
                  <button
                    onClick={() => api.rejectRequest(approval.id)}
                    className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                  >
                    ❌ Reject
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Last Active / Stale Context (Task 15) */}
      {userActivities && userActivities.filter((u) => u.idle_hours > 1).length > 0 && (
        <div className="bg-white rounded-xl shadow-sm p-5 mb-6">
          <h3 className="text-lg font-semibold mb-3">User Activity</h3>
          <div className="space-y-2">
            {userActivities.filter((u) => u.idle_hours > 1).map((user) => (
              <div key={user.user_id} className="flex items-center justify-between p-3 bg-gray-50 rounded-lg">
                <div className="flex items-center gap-3">
                  <span className="text-sm font-mono text-gray-700">{user.user_id}</span>
                  <span className="text-xs text-gray-500">{user.channel}</span>
                </div>
                <span className="text-sm text-amber-600 font-medium">
                  Last active: {user.idle_hours < 24
                    ? `${Math.round(user.idle_hours)}h ago`
                    : `${Math.round(user.idle_hours / 24)}d ago`}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Paired Users per Channel (Task 19) */}
      {pairedUsers && pairedUsers.length > 0 && (
        <div className="bg-white rounded-xl shadow-sm p-5 mb-6">
          <h3 className="text-lg font-semibold mb-3">Paired Users</h3>
          <div className="flex flex-wrap gap-4">
            {Object.entries(usersByChannel).map(([channel, count]) => (
              <div key={channel} className="flex items-center gap-2 px-3 py-2 bg-gray-50 rounded-lg">
                <span className="text-sm font-medium text-gray-700">{channel}</span>
                <span className="text-sm font-bold text-gray-900">{count}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Log Storage (Task 21) */}
      {logStorage && (
        <div className="bg-white rounded-xl shadow-sm p-5 mb-6">
          <h3 className="text-lg font-semibold mb-3">Log Storage</h3>
          <div className="grid grid-cols-3 gap-4">
            <div>
              <div className="text-xs text-gray-500 uppercase">Total Size</div>
              <div className="text-lg font-bold text-gray-900">{formatBytes(logStorage.total_size_bytes)}</div>
            </div>
            <div>
              <div className="text-xs text-gray-500 uppercase">Files</div>
              <div className="text-lg font-bold text-gray-900">{logStorage.file_count}</div>
            </div>
            <div>
              <div className="text-xs text-gray-500 uppercase">Oldest File</div>
              <div className="text-sm font-mono text-gray-700">{logStorage.oldest_file_date}</div>
            </div>
          </div>
        </div>
      )}

      {/* System Info (Task 22) */}
      {systemInfo && (
        <div className="bg-white rounded-xl shadow-sm p-5 mb-6">
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-lg font-semibold">System Info</h3>
            <button
              onClick={() => setConfirmRestart(true)}
              disabled={restarting || (restartStatus?.restarting ?? false)}
              className="px-4 py-2 text-sm font-medium text-gray-600 bg-gray-100 border border-gray-300 rounded-lg hover:bg-gray-200 disabled:opacity-50"
            >
              {restarting || (restartStatus?.restarting ?? false) ? '⏳ Restarting...' : '🔄 Restart'}
            </button>
          </div>

          {/* Restart progress indicator */}
          {restartStatus?.restarting && (
            <div className="mb-4 p-3 bg-yellow-50 border border-yellow-200 rounded-lg">
              <div className="flex items-center gap-2">
                <span className="animate-pulse text-yellow-600">●</span>
                <span className="text-sm font-medium text-yellow-800">
                  Draining... {restartStatus.in_flight_requests} in-flight request{restartStatus.in_flight_requests !== 1 ? 's' : ''} remaining
                </span>
              </div>
              <div className="text-xs text-yellow-600 mt-1">Phase: {restartStatus.phase}</div>
            </div>
          )}

          <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
            <div>
              <div className="text-xs text-gray-500 uppercase">Version</div>
              <div className="text-sm font-mono font-medium">{systemInfo.version}</div>
            </div>
            <div>
              <div className="text-xs text-gray-500 uppercase">Uptime</div>
              <div className="text-sm font-medium">{formatUptime(systemInfo.uptime_secs)}</div>
            </div>
            <div>
              <div className="text-xs text-gray-500 uppercase">Config Path</div>
              <div className="text-xs font-mono text-gray-600 truncate" title={systemInfo.config_path}>{systemInfo.config_path}</div>
            </div>
            <div>
              <div className="text-xs text-gray-500 uppercase">Drain Timeout</div>
              <div className="text-sm font-medium">{systemInfo.drain_timeout_secs}s</div>
            </div>
          </div>
          {systemInfo.build_features && systemInfo.build_features.length > 0 && (
            <div className="mt-3">
              <div className="text-xs text-gray-500 uppercase mb-1">Build Features</div>
              <div className="flex flex-wrap gap-1">
                {systemInfo.build_features.map((f) => (
                  <span key={f} className="px-2 py-0.5 bg-blue-50 text-blue-700 rounded text-xs font-mono">{f}</span>
                ))}
              </div>
            </div>
          )}
          {(systemInfo.docker_status || systemInfo.systemd_status) && (
            <div className="mt-3 flex gap-4">
              {systemInfo.docker_status && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-gray-500">Docker:</span>
                  <StatusBadge status={systemInfo.docker_status} />
                </div>
              )}
              {systemInfo.systemd_status && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-gray-500">Systemd:</span>
                  <StatusBadge status={systemInfo.systemd_status} />
                </div>
              )}
            </div>
          )}
        </div>
      )}

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
