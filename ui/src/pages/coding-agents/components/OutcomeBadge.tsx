import type { TaskOutcome } from '../../../types';

interface OutcomeBadgeProps {
  outcome: TaskOutcome;
}

const outcomeStyles: Record<TaskOutcome, string> = {
  success: 'bg-green-100 text-green-700',
  failure: 'bg-red-100 text-red-700',
  timeout: 'bg-yellow-100 text-yellow-700',
  cancelled: 'bg-gray-100 text-gray-600',
};

const outcomeLabels: Record<TaskOutcome, string> = {
  success: 'Success',
  failure: 'Failure',
  timeout: 'Timeout',
  cancelled: 'Cancelled',
};

export default function OutcomeBadge({ outcome }: OutcomeBadgeProps) {
  return (
    <span className={`px-2 py-0.5 rounded-full text-xs font-medium ${outcomeStyles[outcome]}`}>
      {outcomeLabels[outcome]}
    </span>
  );
}
