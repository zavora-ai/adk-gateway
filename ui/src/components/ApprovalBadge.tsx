interface Props {
  count: number;
  className?: string;
}

export default function ApprovalBadge({ count, className = '' }: Props) {
  if (count === 0) return null;

  return (
    <span
      className={`inline-flex items-center justify-center min-w-[20px] h-5 px-1.5 rounded-full text-xs font-bold bg-orange-500 text-white ${className}`}
      title={`${count} pending approval${count !== 1 ? 's' : ''}`}
    >
      {count}
    </span>
  );
}
