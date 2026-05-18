import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import ConfirmDialog from '../components/ConfirmDialog';
import type { LogEntry, LogFileInfo } from '../types';
import { useState, useEffect, useCallback, useRef } from 'react';

const LEVELS = ['ERROR', 'WARN', 'INFO', 'DEBUG'] as const;
const EVENT_FILTERS = ['all', 'approval', 'rate_limit'] as const;

export default function Logs() {
  const { data, loading, error, refetch } = useApi<LogEntry[]>(() => api.logs(), []);
  const { lastEvent, isConnected } = useWebSocket();
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [visibleLevels, setVisibleLevels] = useState<Set<string>>(new Set(LEVELS));
  const [searchText, setSearchText] = useState('');
  const [eventFilter, setEventFilter] = useState<string>('all');
  const initialized = useRef(false);

  // Log files for download
  const { data: logFiles } = useApi<LogFileInfo[]>(() => api.getLogFiles(), []);
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearing, setClearing] = useState(false);

  // Initialize logs from API data
  useEffect(() => {
    if (data && !initialized.current) {
      setLogs(data);
      initialized.current = true;
    } else if (data && initialized.current) {
      // On refetch, merge
      setLogs(data);
    }
  }, [data]);

  // Append logs from WebSocket
  useEffect(() => {
    if (lastEvent?.type === 'log') {
      const entry: LogEntry = {
        timestamp: lastEvent.timestamp,
        level: lastEvent.level,
        message: lastEvent.message,
        target: lastEvent.target ?? null,
      };
      setLogs((prev) => [...prev, entry]);
    }
  }, [lastEvent]);

  // Polling fallback when WebSocket disconnected
  useEffect(() => {
    if (isConnected) return;
    const interval = setInterval(refetch, 3000);
    return () => clearInterval(interval);
  }, [isConnected, refetch]);

  const toggleLevel = useCallback((level: string) => {
    setVisibleLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) {
        next.delete(level);
      } else {
        next.add(level);
      }
      return next;
    });
  }, []);

  const clearFilters = () => {
    setVisibleLevels(new Set(LEVELS));
    setSearchText('');
    setEventFilter('all');
  };

  const handleClearOldLogs = async () => {
    setClearing(true);
    try {
      await api.clearOldLogs();
      refetch();
    } catch { /* error handled silently */ }
    setClearing(false);
    setConfirmClear(false);
  };

  const isRateLimitEvent = (log: LogEntry) =>
    log.message.toLowerCase().includes('rate_limit') || log.message.toLowerCase().includes('rate limit');

  const isApprovalEvent = (log: LogEntry) =>
    log.message.toLowerCase().includes('approval') || log.target?.toLowerCase().includes('approval');

  const filtered = logs.filter((log) => {
    if (!visibleLevels.has(log.level)) return false;
    if (eventFilter === 'approval' && !isApprovalEvent(log)) return false;
    if (eventFilter === 'rate_limit' && !isRateLimitEvent(log)) return false;
    if (searchText) {
      const q = searchText.toLowerCase();
      const msgMatch = log.message.toLowerCase().includes(q);
      const targetMatch = log.target?.toLowerCase().includes(q) ?? false;
      if (!msgMatch && !targetMatch) return false;
    }
    return true;
  });

  if (loading && logs.length === 0) return <div className="text-gray-400">Loading logs...</div>;
  if (error && logs.length === 0) return <div className="text-red-600">Failed to load logs: {error}</div>;

  return (
    <div>
      <div className="flex items-center justify-between mb-5">
        <h2 className="text-2xl font-semibold">Logs</h2>
        <div className="flex gap-2">
          <button
            onClick={() => setConfirmClear(true)}
            className="px-3 py-1.5 text-xs font-medium text-red-700 bg-red-50 rounded-lg hover:bg-red-100"
          >
            🗑 Clear Old Logs
          </button>
        </div>
      </div>

      {/* Confirm clear dialog */}
      {confirmClear && (
        <ConfirmDialog
          title="Clear Old Logs"
          message="This will delete log files beyond the configured retention period. This action cannot be undone."
          confirmLabel={clearing ? 'Clearing...' : 'Clear'}
          onConfirm={handleClearOldLogs}
          onCancel={() => setConfirmClear(false)}
          destructive
        />
      )}

      {/* Filters */}
      <div className="flex flex-wrap items-center gap-3 mb-4">
        {LEVELS.map((level) => (
          <button
            key={level}
            onClick={() => toggleLevel(level)}
            className={`px-3 py-1.5 text-xs font-semibold rounded-lg border transition-colors ${
              visibleLevels.has(level)
                ? 'border-[var(--color-accent)] bg-[var(--color-accent)] text-white'
                : 'border-gray-300 bg-white text-gray-500'
            }`}
          >
            {level}
          </button>
        ))}

        <span className="text-gray-300">|</span>

        {/* Event type filter */}
        {EVENT_FILTERS.map((ef) => (
          <button
            key={ef}
            onClick={() => setEventFilter(ef)}
            className={`px-3 py-1.5 text-xs font-semibold rounded-lg border transition-colors ${
              eventFilter === ef
                ? ef === 'rate_limit'
                  ? 'border-orange-400 bg-orange-500 text-white'
                  : ef === 'approval'
                    ? 'border-purple-400 bg-purple-500 text-white'
                    : 'border-[var(--color-accent)] bg-[var(--color-accent)] text-white'
                : 'border-gray-300 bg-white text-gray-500'
            }`}
          >
            {ef === 'all' ? 'All Events' : ef === 'approval' ? '🔐 Approvals' : '⚡ Rate Limits'}
          </button>
        ))}

        <input
          type="text"
          value={searchText}
          onChange={(e) => setSearchText(e.target.value)}
          placeholder="Search logs..."
          className="px-3 py-1.5 border border-gray-300 rounded-lg text-sm focus:outline-none focus:border-[var(--color-accent)] min-w-[200px]"
        />

        <button
          onClick={clearFilters}
          className="px-3 py-1.5 text-xs font-medium text-gray-600 bg-gray-100 rounded-lg hover:bg-gray-200"
        >
          Clear Filters
        </button>

        <span className="text-xs text-gray-500 ml-auto">
          {filtered.length} / {logs.length} entries
        </span>
      </div>

      {/* Log table */}
      {filtered.length === 0 ? (
        <div className="text-center py-12 text-gray-400">No log entries match filters</div>
      ) : (
        <div className="bg-white rounded-xl shadow-sm overflow-hidden mb-6">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-44">Timestamp</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-20">Level</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-28">Event</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Message</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-40">Target</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((log, i) => (
                <tr key={i} className={`border-t border-gray-100 hover:bg-gray-50 ${isRateLimitEvent(log) ? 'bg-orange-50/50' : ''}`}>
                  <td className="px-4 py-2 text-xs font-mono text-gray-500">{log.timestamp}</td>
                  <td className="px-4 py-2"><StatusBadge status={log.level} /></td>
                  <td className="px-4 py-2">
                    {isRateLimitEvent(log) && (
                      <span className="inline-block px-2 py-0.5 rounded-full text-xs font-semibold bg-orange-100 text-orange-800">
                        RATE_LIMITED
                      </span>
                    )}
                    {isApprovalEvent(log) && (
                      <span className="inline-block px-2 py-0.5 rounded-full text-xs font-semibold bg-purple-100 text-purple-800">
                        APPROVAL
                      </span>
                    )}
                  </td>
                  <td className="px-4 py-2 text-sm break-all">{log.message}</td>
                  <td className="px-4 py-2 text-xs text-gray-500 font-mono">{log.target || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Log Files (Download) */}
      {logFiles && logFiles.length > 0 && (
        <div className="bg-white rounded-xl shadow-sm p-5">
          <h3 className="text-lg font-semibold mb-3">Log Files</h3>
          <div className="space-y-2">
            {logFiles.map((file) => (
              <div key={file.filename} className="flex items-center justify-between px-3 py-2 bg-gray-50 rounded-lg">
                <div className="flex items-center gap-3">
                  <span className="text-sm font-mono text-gray-700">{file.filename}</span>
                  <span className="text-xs text-gray-400">{(file.size_bytes / 1024).toFixed(1)} KB</span>
                </div>
                <a
                  href={api.downloadLogFile(file.filename)}
                  download
                  className="px-3 py-1 text-xs font-medium text-blue-700 bg-blue-50 rounded-lg hover:bg-blue-100 no-underline"
                >
                  ⬇ Download
                </a>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
