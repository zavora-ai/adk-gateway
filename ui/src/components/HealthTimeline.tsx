import type { HealthComponent, HealthEvent } from '../types';

interface Props {
  component: HealthComponent;
  events: HealthEvent[];
  onClose: () => void;
}

export default function HealthTimeline({ component, events, onClose }: Props) {
  const componentEvents = events.filter((e) => e.component === component.name);
  const statusColor =
    component.status === 'healthy'
      ? 'bg-green-500'
      : component.status === 'degraded'
        ? 'bg-yellow-500'
        : 'bg-red-500';

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white rounded-xl shadow-xl max-w-lg w-full p-6 max-h-[80vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            <span className={`inline-block w-3 h-3 rounded-full ${statusColor}`} />
            <h3 className="text-lg font-semibold">{component.name}</h3>
          </div>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 text-xl leading-none"
            aria-label="Close"
          >
            ×
          </button>
        </div>

        {/* Current Status */}
        <div className="bg-gray-50 rounded-lg p-4 mb-4 space-y-2">
          <div className="flex justify-between text-sm">
            <span className="text-gray-600">Status</span>
            <span className="font-medium capitalize">{component.status}</span>
          </div>
          <div className="flex justify-between text-sm">
            <span className="text-gray-600">Last Check</span>
            <span className="font-mono text-xs">{component.last_check}</span>
          </div>
          <div className="flex justify-between text-sm">
            <span className="text-gray-600">Consecutive Failures</span>
            <span className="font-medium">{component.consecutive_failures}</span>
          </div>
          {component.latency_ms !== undefined && (
            <div className="flex justify-between text-sm">
              <span className="text-gray-600">Latency</span>
              <span className="font-mono text-xs">{component.latency_ms}ms</span>
            </div>
          )}
        </div>

        {/* Timeline */}
        <h4 className="text-sm font-semibold text-gray-700 mb-3">Event History</h4>
        {componentEvents.length === 0 ? (
          <div className="text-sm text-gray-400 text-center py-4">No events recorded</div>
        ) : (
          <div className="space-y-3">
            {componentEvents.map((event, i) => (
              <div key={i} className="flex items-start gap-3">
                <div className="flex-shrink-0 mt-1">
                  <span
                    className={`inline-block w-2.5 h-2.5 rounded-full ${
                      event.event === 'alert' ? 'bg-red-500' : 'bg-green-500'
                    }`}
                  />
                </div>
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className={`text-xs font-semibold uppercase ${
                      event.event === 'alert' ? 'text-red-700' : 'text-green-700'
                    }`}>
                      {event.event}
                    </span>
                    <span className="text-xs text-gray-400 font-mono">{event.timestamp}</span>
                  </div>
                  <p className="text-sm text-gray-600 mt-0.5">{event.message}</p>
                </div>
              </div>
            ))}
          </div>
        )}

        <div className="mt-6 flex justify-end">
          <button
            onClick={onClose}
            className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-lg hover:bg-gray-200"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
