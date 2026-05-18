/**
 * Unit tests for NotificationStack component — Task 2.5
 *
 * Tests verify:
 * - Renders notifications with correct styling per type
 * - Auto-dismisses success notifications after 3 seconds
 * - Error notifications require manual dismissal
 * - Warning notifications render with correct styling
 * - Dismiss button calls onDismiss with correct id
 * - Returns null when no notifications
 *
 * @vitest-environment jsdom
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import NotificationStack from '../NotificationStack';
import type { Notification } from '../NotificationStack';

describe('NotificationStack', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders nothing when notifications array is empty', () => {
    const { container } = render(
      <NotificationStack notifications={[]} onDismiss={vi.fn()} />
    );
    expect(container.firstChild).toBeNull();
  });

  it('renders success notification with green styling', () => {
    const notifications: Notification[] = [
      { id: '1', type: 'success', message: 'Agent created', persistent: false },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={vi.fn()} />);

    const alert = screen.getByRole('alert');
    expect(alert).toHaveClass('bg-green-50', 'border-green-300', 'text-green-800');
    expect(screen.getByText('Agent created')).toBeInTheDocument();
  });

  it('renders error notification with red styling', () => {
    const notifications: Notification[] = [
      { id: '1', type: 'error', message: 'Failed to save', persistent: true },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={vi.fn()} />);

    const alert = screen.getByRole('alert');
    expect(alert).toHaveClass('bg-red-50', 'border-red-300', 'text-red-800');
    expect(screen.getByText('Failed to save')).toBeInTheDocument();
  });

  it('renders warning notification with yellow styling', () => {
    const notifications: Notification[] = [
      { id: '1', type: 'warning', message: 'Cost threshold exceeded', persistent: false },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={vi.fn()} />);

    const alert = screen.getByRole('alert');
    expect(alert).toHaveClass('bg-yellow-50', 'border-yellow-300', 'text-yellow-800');
    expect(screen.getByText('Cost threshold exceeded')).toBeInTheDocument();
  });

  it('auto-dismisses success notifications after 3 seconds', () => {
    const onDismiss = vi.fn();
    const notifications: Notification[] = [
      { id: 'success-1', type: 'success', message: 'Done', persistent: false },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={onDismiss} />);

    expect(onDismiss).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(3000);
    });

    expect(onDismiss).toHaveBeenCalledWith('success-1');
  });

  it('does not auto-dismiss error notifications', () => {
    const onDismiss = vi.fn();
    const notifications: Notification[] = [
      { id: 'error-1', type: 'error', message: 'Something broke', persistent: true },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={onDismiss} />);

    act(() => {
      vi.advanceTimersByTime(5000);
    });

    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('calls onDismiss when dismiss button is clicked', () => {
    const onDismiss = vi.fn();
    const notifications: Notification[] = [
      { id: 'err-1', type: 'error', message: 'Error occurred', persistent: true },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={onDismiss} />);

    fireEvent.click(screen.getByLabelText('Dismiss notification'));

    expect(onDismiss).toHaveBeenCalledWith('err-1');
  });

  it('renders multiple notifications stacked', () => {
    const notifications: Notification[] = [
      { id: '1', type: 'success', message: 'Created', persistent: false },
      { id: '2', type: 'error', message: 'Failed', persistent: true },
      { id: '3', type: 'warning', message: 'Warning', persistent: false },
    ];

    render(<NotificationStack notifications={notifications} onDismiss={vi.fn()} />);

    expect(screen.getAllByRole('alert')).toHaveLength(3);
    expect(screen.getByText('Created')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
    expect(screen.getByText('Warning')).toBeInTheDocument();
  });

  it('positions the stack fixed at top-right', () => {
    const notifications: Notification[] = [
      { id: '1', type: 'success', message: 'Test', persistent: false },
    ];

    const { container } = render(
      <NotificationStack notifications={notifications} onDismiss={vi.fn()} />
    );

    const stack = container.firstChild as HTMLElement;
    expect(stack).toHaveClass('fixed', 'top-4', 'right-4');
  });
});
