import { useEffect, useRef } from 'react';

export interface Notification {
  id: string;
  type: 'success' | 'error' | 'warning';
  message: string;
  persistent: boolean;
}

interface NotificationStackProps {
  notifications: Notification[];
  onDismiss: (id: string) => void;
}

const typeStyles: Record<Notification['type'], string> = {
  success: 'bg-green-50 border-green-300 text-green-800',
  error: 'bg-red-50 border-red-300 text-red-800',
  warning: 'bg-yellow-50 border-yellow-300 text-yellow-800',
};

const dismissButtonStyles: Record<Notification['type'], string> = {
  success: 'text-green-600 hover:text-green-800',
  error: 'text-red-600 hover:text-red-800',
  warning: 'text-yellow-600 hover:text-yellow-800',
};

export default function NotificationStack({ notifications, onDismiss }: NotificationStackProps) {
  const timersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  useEffect(() => {
    const currentTimers = timersRef.current;

    notifications.forEach((notification) => {
      if (notification.type === 'success' && !notification.persistent && !currentTimers.has(notification.id)) {
        const timer = setTimeout(() => {
          onDismiss(notification.id);
          currentTimers.delete(notification.id);
        }, 3000);
        currentTimers.set(notification.id, timer);
      }
    });

    // Clean up timers for notifications that no longer exist
    currentTimers.forEach((timer, id) => {
      if (!notifications.find((n) => n.id === id)) {
        clearTimeout(timer);
        currentTimers.delete(id);
      }
    });

    return () => {
      currentTimers.forEach((timer) => clearTimeout(timer));
    };
  }, [notifications, onDismiss]);

  if (notifications.length === 0) {
    return null;
  }

  return (
    <div className="fixed top-4 right-4 z-50 flex flex-col gap-2 max-w-sm w-full">
      {notifications.map((notification) => (
        <div
          key={notification.id}
          role="alert"
          className={`border rounded-lg p-3 shadow-md flex items-start gap-2 ${typeStyles[notification.type]}`}
        >
          <p className="text-sm flex-1">{notification.message}</p>
          <button
            type="button"
            onClick={() => onDismiss(notification.id)}
            className={`text-sm font-medium shrink-0 ${dismissButtonStyles[notification.type]}`}
            aria-label="Dismiss notification"
          >
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
