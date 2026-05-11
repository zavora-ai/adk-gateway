/**
 * UI Tests for Agents page — Task 6.6, 8.8
 *
 * Tests verify:
 * - Agent state updates on WebSocket events (Task 6.6)
 * - Polling fallback when WebSocket is disconnected (Task 6.6)
 * - Configure modal pre-population (Task 8.8)
 * - Configure modal submission and success handling (Task 8.8)
 * - Configure modal error handling (Task 8.8)
 *
 * @vitest-environment jsdom
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';

// ── Hoisted mutable state (accessible inside vi.mock factories) ────

const { mockRefetch, mockState } = vi.hoisted(() => ({
  mockRefetch: vi.fn(),
  mockState: {
    isConnected: true,
    lastEvent: null as unknown,
  },
}));

const agentData = [
  {
    id: 'agent-1',
    name: 'Test Agent',
    description: 'A test agent',
    agent_type: 'a2a',
    state: 'stopped',
    port: null,
    model: 'gpt-4o',
    tools: ['web_search'],
    instruction: 'You are a helpful assistant.',
    api_key_env: 'OPENAI_API_KEY',
    auto_start: false,
    channel_bindings: [{ channel_type: 'slack', account_id: 'C12345' }],
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
  },
];

vi.mock('../../hooks/useApi', () => ({
  useApi: () => ({
    data: [
      {
        id: 'agent-1',
        name: 'Test Agent',
        description: 'A test agent',
        agent_type: 'a2a',
        state: 'stopped',
        port: null,
        model: 'gpt-4o',
        tools: ['web_search'],
        instruction: 'You are a helpful assistant.',
        api_key_env: 'OPENAI_API_KEY',
        auto_start: false,
        channel_bindings: [{ channel_type: 'slack', account_id: 'C12345' }],
        created_at: '2024-01-01T00:00:00Z',
        updated_at: '2024-01-01T00:00:00Z',
      },
    ],
    loading: false,
    error: null,
    refetch: mockRefetch,
  }),
}));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({
    lastEvent: mockState.lastEvent,
    isConnected: mockState.isConnected,
  }),
}));

vi.mock('../../api/client', () => ({
  api: {
    agents: vi.fn().mockResolvedValue({ ok: true, data: [] }),
    startAgent: vi.fn().mockResolvedValue({ ok: true }),
    stopAgent: vi.fn().mockResolvedValue({ ok: true }),
    deleteAgent: vi.fn().mockResolvedValue({ ok: true }),
    agentLogs: vi.fn().mockResolvedValue({ ok: true, data: { logs: [] } }),
    post: vi.fn().mockResolvedValue({ ok: true, message: 'Configuration updated.' }),
    delegationList: vi.fn().mockResolvedValue({ ok: true, data: [] }),
    delegationAdd: vi.fn().mockResolvedValue({ ok: true }),
    delegationRemove: vi.fn().mockResolvedValue({ ok: true }),
  },
}));

import Agents from '../Agents';

// ── Task 6.6: WebSocket Agent State Updates ────────────────────────

describe('Real-Time Agent State Updates', () => {
  beforeEach(() => {
    mockState.isConnected = true;
    mockState.lastEvent = null;
    mockRefetch.mockClear();
  });

  it('updates agent state when receiving agent_state WebSocket event', async () => {
    mockState.lastEvent = { type: 'agent_state', agent_id: 'agent-1', state: 'running' };

    render(<Agents />);

    await waitFor(() => {
      expect(screen.getByText('running')).toBeInTheDocument();
    });
  });

  it('shows connection indicator as connected when WebSocket is active', () => {
    mockState.isConnected = true;
    render(<Agents />);
    expect(screen.getByText('Live')).toBeInTheDocument();
  });

  it('shows connection indicator as reconnecting when WebSocket is disconnected', () => {
    mockState.isConnected = false;
    render(<Agents />);
    expect(screen.getByText('Reconnecting')).toBeInTheDocument();
  });

  it('falls back to polling when WebSocket is disconnected', async () => {
    vi.useFakeTimers();
    mockState.isConnected = false;

    render(<Agents />);

    // Advance time by 5 seconds to trigger polling
    act(() => {
      vi.advanceTimersByTime(5000);
    });

    expect(mockRefetch).toHaveBeenCalled();

    // Advance again
    act(() => {
      vi.advanceTimersByTime(5000);
    });

    expect(mockRefetch).toHaveBeenCalledTimes(2);

    vi.useRealTimers();
  });

  it('does not poll when WebSocket is connected', () => {
    vi.useFakeTimers();
    mockState.isConnected = true;

    render(<Agents />);

    act(() => {
      vi.advanceTimersByTime(10000);
    });

    expect(mockRefetch).not.toHaveBeenCalled();

    vi.useRealTimers();
  });
});

// ── Task 8.8: Configure Modal Tests ───────────────────────────────

describe('Configure Modal', () => {
  beforeEach(() => {
    mockState.isConnected = true;
    mockState.lastEvent = null;
  });

  it('displays Configure button for each agent', () => {
    render(<Agents />);
    expect(screen.getByText('Configure')).toBeInTheDocument();
  });

  it('opens configure modal pre-populated with agent config', async () => {
    render(<Agents />);

    fireEvent.click(screen.getByText('Configure'));

    await waitFor(() => {
      expect(screen.getByText('Configure: Test Agent')).toBeInTheDocument();
    });

    // Check pre-populated values
    const modelInput = screen.getByDisplayValue('gpt-4o');
    expect(modelInput).toBeInTheDocument();

    const instructionInput = screen.getByDisplayValue('You are a helpful assistant.');
    expect(instructionInput).toBeInTheDocument();

    const toolsInput = screen.getByDisplayValue('web_search');
    expect(toolsInput).toBeInTheDocument();

    const bindingsInput = screen.getByDisplayValue('slack:C12345');
    expect(bindingsInput).toBeInTheDocument();
  });

  it('submits configure form and shows success', async () => {
    const { api } = await import('../../api/client');
    (api.post as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ ok: true, message: 'Updated.' });

    render(<Agents />);

    fireEvent.click(screen.getByText('Configure'));

    await waitFor(() => {
      expect(screen.getByText('Configure: Test Agent')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      expect(screen.getByText(/configuration updated/i)).toBeInTheDocument();
    });
  });

  it('shows error and keeps modal open on failure', async () => {
    const { api } = await import('../../api/client');
    (api.post as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ok: false,
      message: 'Agent is locked',
    });

    render(<Agents />);

    fireEvent.click(screen.getByText('Configure'));

    await waitFor(() => {
      expect(screen.getByText('Configure: Test Agent')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      // Error message shown
      expect(screen.getByText('Agent is locked')).toBeInTheDocument();
      // Modal still open
      expect(screen.getByText('Configure: Test Agent')).toBeInTheDocument();
    });
  });
});
