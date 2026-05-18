/**
 * Unit tests for ConnectionBadge component
 *
 * Tests verify:
 * - Correct color classes for each connection status
 * - Correct label text for each status
 *
 * @vitest-environment jsdom
 */

import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import ConnectionBadge from '../ConnectionBadge';

describe('ConnectionBadge', () => {
  it('renders green badge for connected status', () => {
    render(<ConnectionBadge status="connected" />);
    const badge = screen.getByText('Connected');
    expect(badge).toBeDefined();
    expect(badge.className).toContain('bg-green-100');
    expect(badge.className).toContain('text-green-700');
  });

  it('renders gray badge for disconnected status', () => {
    render(<ConnectionBadge status="disconnected" />);
    const badge = screen.getByText('Disconnected');
    expect(badge).toBeDefined();
    expect(badge.className).toContain('bg-gray-100');
    expect(badge.className).toContain('text-gray-600');
  });

  it('renders red badge for error status', () => {
    render(<ConnectionBadge status="error" />);
    const badge = screen.getByText('Error');
    expect(badge).toBeDefined();
    expect(badge.className).toContain('bg-red-100');
    expect(badge.className).toContain('text-red-700');
  });

  it('applies base badge classes for all statuses', () => {
    const { rerender } = render(<ConnectionBadge status="connected" />);
    let badge = screen.getByText('Connected');
    expect(badge.className).toContain('px-2');
    expect(badge.className).toContain('py-0.5');
    expect(badge.className).toContain('rounded-full');
    expect(badge.className).toContain('text-xs');
    expect(badge.className).toContain('font-medium');

    rerender(<ConnectionBadge status="disconnected" />);
    badge = screen.getByText('Disconnected');
    expect(badge.className).toContain('px-2');
    expect(badge.className).toContain('rounded-full');

    rerender(<ConnectionBadge status="error" />);
    badge = screen.getByText('Error');
    expect(badge.className).toContain('px-2');
    expect(badge.className).toContain('rounded-full');
  });
});
