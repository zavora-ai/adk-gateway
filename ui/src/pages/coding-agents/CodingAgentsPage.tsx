import { useState, useEffect, useCallback } from 'react';
import { Outlet } from 'react-router-dom';
import { useWebSocket } from '../../hooks/useWebSocket';
import NotificationStack from './components/NotificationStack';
import type { Notification } from './components/NotificationStack';

export default function CodingAgentsPage() {
  const { lastEvent, isConnected } = useWebSocket();
  const [notifications, setNotifications] = useState<Notification[]>([]);

  useEffect(() => {
    if (!lastEvent) return;
    if (lastEvent.type === 'coding_agent_cost_warning') {
      const event = lastEvent as Extract<typeof lastEvent, { type: 'coding_agent_cost_warning' }>;
      const notification: Notification = {
        id: `cost-warning-${Date.now()}`,
        type: 'warning',
        message: `Cost warning: Agent spending $${event.current_usd.toFixed(2)} has exceeded threshold of $${event.threshold_usd.toFixed(2)}`,
        persistent: true,
      };
      setNotifications((prev) => [...prev, notification]);
    }
  }, [lastEvent]);

  const handleDismiss = useCallback((id: string) => {
    setNotifications((prev) => prev.filter((n) => n.id !== id));
  }, []);

  return (
    <div className="relative">
      {!isConnected && (
        <div className="bg-yellow-50 border-b border-yellow-200 px-4 py-2 text-sm text-yellow-700 text-center">
          Real-time updates paused — reconnecting…
        </div>
      )}
      <Outlet />
      <NotificationStack notifications={notifications} onDismiss={handleDismiss} />
    </div>
  );
}
