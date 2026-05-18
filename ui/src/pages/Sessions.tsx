import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import ConfirmDialog from '../components/ConfirmDialog';
import type { SessionInfo } from '../types';
import { useState, useEffect } from 'react';

export default function Sessions() {
  const { data, loading, error, refetch } = useApi<SessionInfo[]>(() => api.sessions(), []);
  const { lastEvent, isConnected } = useWebSocket();
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [terminateTarget, setTerminateTarget] = useState<string | null>(null);
  const [userFilter, setUserFilter] = useState<string>('');

  // Auto-refresh from WebSocket dashboard events
  useEffect(() => {
    if (lastEvent?.type === 'dashboard') {
      refetch();
    }
  }, [lastEvent, refetch]);

  // Polling fallback when WebSocket disconnected
  useEffect(() => {
    if (isConnected) return;
    const interval = setInterval(refetch, 3000);
    return () => clearInterval(interval);
  }, [isConnected, refetch]);

  const handleTerminate = async (id: string) => {
    await api.terminateSession(id);
    setTerminateTarget(null);
    refetch();
  };

  if (loading) return <div className="text-gray-400">Loading sessions...</div>;
  if (error) return <div className="text-red-600">Failed to load sessions: {error}</div>;

  const sessions = data || [];
  const filteredSessions = userFilter
    ? sessions.filter((s) => s.user_id.toLowerCase().includes(userFilter.toLowerCase()))
    : sessions;

  // Get unique user IDs for filter suggestions
  const uniqueUsers = [...new Set(sessions.map((s) => s.user_id))];

  return (
    <div>
      <div className="flex items-center gap-3 mb-5">
        <h2 className="text-2xl font-semibold">Sessions</h2>
        <span className="bg-gray-200 text-gray-700 text-xs font-semibold px-2.5 py-0.5 rounded-full">
          {filteredSessions.length}{userFilter ? ` / ${sessions.length}` : ''} total
        </span>
      </div>

      {/* User filter (Task 19) */}
      <div className="flex items-center gap-3 mb-4">
        <input
          type="text"
          value={userFilter}
          onChange={(e) => setUserFilter(e.target.value)}
          placeholder="Filter by user ID..."
          list="user-suggestions"
          className="px-3 py-1.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] min-w-[250px]"
        />
        <datalist id="user-suggestions">
          {uniqueUsers.map((u) => <option key={u} value={u} />)}
        </datalist>
        {userFilter && (
          <button
            onClick={() => setUserFilter('')}
            className="px-3 py-1.5 text-xs font-medium text-gray-600 bg-gray-100 rounded-lg hover:bg-gray-200"
          >
            Clear
          </button>
        )}
      </div>

      {filteredSessions.length === 0 ? (
        <div className="text-center py-12 text-gray-400">
          {userFilter ? `No sessions found for "${userFilter}"` : 'No active sessions'}
        </div>
      ) : (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Session ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">User ID</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Channel</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Last Activity</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
              </tr>
            </thead>
            <tbody>
              {filteredSessions.map((s) => (
                <SessionRow
                  key={s.session_id}
                  session={s}
                  expanded={expandedId === s.session_id}
                  onToggle={() => setExpandedId(expandedId === s.session_id ? null : s.session_id)}
                  onTerminate={() => setTerminateTarget(s.session_id)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      {terminateTarget && (
        <ConfirmDialog
          title="Terminate Session"
          message={`Are you sure you want to terminate session ${terminateTarget}?`}
          confirmLabel="Terminate"
          destructive
          onConfirm={() => handleTerminate(terminateTarget)}
          onCancel={() => setTerminateTarget(null)}
        />
      )}
    </div>
  );
}

function SessionRow({
  session,
  expanded,
  onToggle,
  onTerminate,
}: {
  session: SessionInfo;
  expanded: boolean;
  onToggle: () => void;
  onTerminate: () => void;
}) {
  return (
    <>
      <tr
        className="border-t border-gray-100 hover:bg-gray-50 cursor-pointer"
        onClick={onToggle}
      >
        <td className="px-4 py-3 text-sm font-mono">{session.session_id}</td>
        <td className="px-4 py-3 text-sm">{session.user_id}</td>
        <td className="px-4 py-3 text-sm">{session.channel_type}</td>
        <td className="px-4 py-3 text-sm text-gray-500">{session.last_activity}</td>
        <td className="px-4 py-3">
          <button
            onClick={(e) => { e.stopPropagation(); onTerminate(); }}
            className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
          >
            Terminate
          </button>
        </td>
      </tr>
      {expanded && (
        <tr className="bg-gray-50">
          <td colSpan={5} className="px-6 py-4 text-sm text-gray-600">
            <div className="grid grid-cols-2 gap-2">
              <div><strong>Session ID:</strong> {session.session_id}</div>
              <div><strong>User ID:</strong> {session.user_id}</div>
              <div><strong>Channel:</strong> {session.channel_type}</div>
              <div><strong>Last Activity:</strong> {session.last_activity}</div>
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
