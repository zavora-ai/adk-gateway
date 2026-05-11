import { Link } from 'react-router-dom';

interface Props {
  label: string;
  value: string | number;
  color?: string;
  linkTo?: string;
}

export default function MetricCard({ label, value, color, linkTo }: Props) {
  const content = (
    <div className="bg-white rounded-xl p-5 shadow-sm min-w-[180px] hover:shadow-md transition-shadow">
      <div className="text-xs uppercase tracking-wide text-gray-500 mb-1">{label}</div>
      <div className={`text-3xl font-bold ${color || 'text-gray-900'}`}>{value}</div>
    </div>
  );

  if (linkTo) {
    return <Link to={linkTo} className="no-underline">{content}</Link>;
  }
  return content;
}
