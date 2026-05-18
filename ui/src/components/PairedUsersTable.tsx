import { useState } from 'react';
import type { PairedUser } from '../types';
import { api } from '../api/client';
import ConfirmDialog from './ConfirmDialog';
import StatusBadge from './StatusBadge';

interface Props {
  users: PairedUser[];
  onRefresh: () => void;
}

export default function PairedUsersTable({ users, onRefresh }: Props) {
  const [confirmUnpair, setConfirmUnpair] = useState<string | null>(null);
  const [editingHeartbeat, setEditingHeartbeat] = useState<string | null>(null);
  const [heartbeatInterval, setHeartbeatInterval] = useState<string>('');

  const handleUnpair = async (userId: string) => {
    try {
      await api.unpairUser(userId);
      onRefresh();
    } catch {
      // Error handled silently
    }
    setConfirmUnpair(null);
  };

  const handleToggleHeartbeat = async (user: PairedUser) => {
    try {
      await api.updateUserHeartbeat(user.user_id, {
        enabled: !user.heartbeat_enabled,
        interval_secs: user.heartbeat_interval_secs,
      });
      onRefresh();
    } catch {
      // Error handled silently
    }
  };

  const handleSaveInterval = async (userId: string) => {
    const secs = parseInt(heartbeatInterval, 10);
    if (isNaN(secs) || secs < 60) return;
    try {
      await api.updateUserHeartbeat(userId, { enabled: true, interval_secs: secs });
      onRefresh();
    } catch {
      // Error handled silently
    }
    setEditingHeartbeat(null);
  };

  if (users.length === 0) {
    return (
      <div className="text-center py-8 text-gray-400 bg-white rounded-xl shadow-sm">
        No paired users
      </div>
    );
  }

  return (
    <>
      {confirmUnpair && (
        <ConfirmDialog
          title="Unpair User"
          message={`Are you sure you want to unpair user '${confirmUnpair}'? This will stop their heartbeat and session.`}
          onConfirm={() => handleUnpair(confirmUnpair)}
          onCancel={() => setConfirmUnpair(null)}
          destructive
        />
      )}

      <div className="bg-white rounded-xl shadow-sm overflow-hidden">
        <table className="w-full">
          <thead>
            <tr className="bg-gray-50">
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">User ID</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Channel</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Paired At</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Last Active</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Heartbeat</th>
              <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Actions</th>
            </tr>
          </thead>
          <tbody>
            {users.map((user) => (
              <tr key={user.user_id} className="border-t border-gray-100 hover:bg-gray-50">
                <td className="px-4 py-3 text-sm font-mono">{user.user_id}</td>
                <td className="px-4 py-3 text-sm">{user.channel}</td>
                <td className="px-4 py-3 text-xs text-gray-500 font-mono">{user.paired_at}</td>
                <td className="px-4 py-3 text-xs text-gray-500 font-mono">{user.last_active}</td>
                <td className="px-4 py-3">
                  <div className="flex items-center gap-2">
                    <StatusBadge status={user.heartbeat_status} />
                    {editingHeartbeat === user.user_id ? (
                      <div className="flex items-center gap-1">
                        <input
                          type="number"
                          value={heartbeatInterval}
                          onChange={(e) => setHeartbeatInterval(e.target.value)}
                          min="60"
                          className="w-20 px-2 py-1 border border-gray-300 rounded text-xs"
                          placeholder="secs"
                        />
                        <button
                          onClick={() => handleSaveInterval(user.user_id)}
                          className="px-2 py-1 text-xs font-medium text-green-700 bg-green-50 rounded hover:bg-green-100"
                        >
                          ✓
                        </button>
                        <button
                          onClick={() => setEditingHeartbeat(null)}
                          className="px-2 py-1 text-xs font-medium text-gray-600 bg-gray-100 rounded hover:bg-gray-200"
                        >
                          ✗
                        </button>
                      </div>
                    ) : (
                      <button
                        onClick={() => {
                          setEditingHeartbeat(user.user_id);
                          setHeartbeatInterval(String(user.heartbeat_interval_secs || 3600));
                        }}
                        className="text-xs text-gray-400 hover:text-gray-600"
                        title="Edit interval"
                      >
                        ⚙
                      </button>
                    )}
                  </div>
                </td>
                <td className="px-4 py-3 flex gap-2">
                  <button
                    onClick={() => handleToggleHeartbeat(user)}
                    className={`px-3 py-1 text-xs font-medium rounded-lg ${
                      user.heartbeat_enabled
                        ? 'text-yellow-700 bg-yellow-50 hover:bg-yellow-100'
                        : 'text-green-700 bg-green-50 hover:bg-green-100'
                    }`}
                  >
                    {user.heartbeat_enabled ? 'Pause HB' : 'Enable HB'}
                  </button>
                  <button
                    onClick={() => setConfirmUnpair(user.user_id)}
                    className="px-3 py-1 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
                  >
                    Unpair
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}
