const colorMap: Record<string, string> = {
  connected: 'bg-green-100 text-green-800',
  healthy: 'bg-green-100 text-green-800',
  running: 'bg-green-100 text-green-800',
  success: 'bg-green-100 text-green-800',
  active: 'bg-green-100 text-green-800',

  reconnecting: 'bg-yellow-100 text-yellow-800',
  degrading: 'bg-yellow-100 text-yellow-800',
  starting: 'bg-blue-100 text-blue-800',
  stopping: 'bg-yellow-100 text-yellow-800',
  stopped: 'bg-yellow-100 text-yellow-800',
  created: 'bg-gray-100 text-gray-700',

  failed: 'bg-red-100 text-red-800',
  degraded: 'bg-red-100 text-red-800',
  error: 'bg-red-100 text-red-800',

  disconnected: 'bg-gray-100 text-gray-600',
  disabled: 'bg-gray-100 text-gray-600',
  cancelled: 'bg-gray-100 text-gray-600',
};

const levelMap: Record<string, string> = {
  ERROR: 'bg-red-100 text-red-800',
  WARN: 'bg-yellow-100 text-yellow-800',
  INFO: 'bg-blue-100 text-blue-800',
  DEBUG: 'bg-gray-100 text-gray-600',
};

interface Props {
  status: string;
  className?: string;
}

export default function StatusBadge({ status, className = '' }: Props) {
  const lower = status.toLowerCase();
  const colors = colorMap[lower] || levelMap[status] || 'bg-gray-100 text-gray-600';
  return (
    <span className={`inline-block px-2.5 py-0.5 rounded-full text-xs font-semibold ${colors} ${className}`}>
      {status}
    </span>
  );
}
