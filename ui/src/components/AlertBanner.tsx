import { useEffect, useState } from 'react';

interface Props {
  type: 'success' | 'error' | 'info';
  message: string;
  autoDismiss?: number; // ms, 0 = no auto-dismiss
  onDismiss?: () => void;
}

export default function AlertBanner({ type, message, autoDismiss = 4000, onDismiss }: Props) {
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    if (autoDismiss > 0) {
      const timer = setTimeout(() => {
        setVisible(false);
        onDismiss?.();
      }, autoDismiss);
      return () => clearTimeout(timer);
    }
  }, [autoDismiss, onDismiss]);

  if (!visible) return null;

  const colors = {
    success: 'bg-green-50 border-green-200 text-green-800',
    error: 'bg-red-50 border-red-200 text-red-800',
    info: 'bg-blue-50 border-blue-200 text-blue-800',
  };

  return (
    <div className={`border rounded-lg px-4 py-3 mb-4 text-sm flex items-center justify-between ${colors[type]}`}>
      <span>{message}</span>
      <button
        onClick={() => { setVisible(false); onDismiss?.(); }}
        className="ml-4 text-lg leading-none opacity-50 hover:opacity-100"
      >
        ×
      </button>
    </div>
  );
}
