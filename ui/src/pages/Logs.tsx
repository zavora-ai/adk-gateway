import { useApi } from '../hooks/useApi';
import { useWebSocket } from '../hooks/useWebSocket';
import { api } from '../api/client';
import StatusBadge from '../components/StatusBadge';
import type { LogEntry } from '../types';
import { useState, useEffect, useCallback, useRef } from 'react';

const LEVELS = ['ERROR', 'WARN', 'INFO', 'DEBUG'] as const;

export default function Logs() {
  const { data, loading, error, refetch } = useApi<LogEntry[]>(() => api.logs(), []);
  const { lastEvent, isConnected } = useWebSocket();
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [visibleLevels, setVisibleLevels] = useState<Set<string>>(new Set(LEVELS));
  const [searchText, setSearchText] = useState('');
  const initialized = useRef(false);

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
  };

  const filtered = logs.filter((log) => {
    if (!visibleLevels.has(log.level)) return false;
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
      <h2 className="text-2xl font-semibold mb-5">Logs</h2>

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
        <div className="bg-white rounded-xl shadow-sm overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="bg-gray-50">
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-44">Timestamp</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-20">Level</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500">Message</th>
                <th className="text-left px-4 py-3 text-xs uppercase tracking-wide text-gray-500 w-40">Target</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((log, i) => (
                <tr key={i} className="border-t border-gray-100 hover:bg-gray-50">
                  <td className="px-4 py-2 text-xs font-mono text-gray-500">{log.timestamp}</td>
                  <td className="px-4 py-2"><StatusBadge status={log.level} /></td>
                  <td className="px-4 py-2 text-sm break-all">{log.message}</td>
                  <td className="px-4 py-2 text-xs text-gray-500 font-mono">{log.target || '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
