import type { CodingAgentConnectionStatus } from '../../../types';

interface ConnectionBadgeProps {
  status: CodingAgentConnectionStatus;
}

const statusConfig: Record<CodingAgentConnectionStatus, { label: string; classes: string }> = {
  connected: { label: 'Connected', classes: 'bg-green-100 text-green-700' },
  disconnected: { label: 'Disconnected', classes: 'bg-gray-100 text-gray-600' },
  error: { label: 'Error', classes: 'bg-red-100 text-red-700' },
};

export default function ConnectionBadge({ status }: ConnectionBadgeProps) {
  const { label, classes } = statusConfig[status];

  return (
    <span className={`px-2 py-0.5 rounded-full text-xs font-medium ${classes}`}>
      {label}
    </span>
  );
}
