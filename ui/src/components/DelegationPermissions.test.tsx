import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import DelegationPermissions, { wouldCreateCycle } from './DelegationPermissions';
import type { AgentRecord, DelegationRule } from '../types';

// Mock the API client
vi.mock('../api/client', () => ({
  api: {
    delegationList: vi.fn(),
    delegationAdd: vi.fn(),
    delegationRemove: vi.fn(),
  },
}));

import { api } from '../api/client';

const mockAgents: AgentRecord[] = [
  {
    id: 'agent-a',
    name: 'Agent A',
    description: '',
    agent_type: 'Llm',
    state: 'running',
    port: null,
    model: 'gpt-4o',
    tools: [],
    instruction: '',
    api_key_env: '',
    auto_start: false,
    channel_bindings: [],
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
  },
  {
    id: 'agent-b',
    name: 'Agent B',
    description: '',
    agent_type: 'Llm',
    state: 'stopped',
    port: null,
    model: 'gpt-4o',
    tools: [],
    instruction: '',
    api_key_env: '',
    auto_start: false,
    channel_bindings: [],
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
  },
  {
    id: 'agent-c',
    name: 'Agent C',
    description: '',
    agent_type: 'Llm',
    state: 'running',
    port: null,
    model: 'gpt-4o',
    tools: [],
    instruction: '',
    api_key_env: '',
    auto_start: false,
    channel_bindings: [],
    created_at: '2024-01-01T00:00:00Z',
    updated_at: '2024-01-01T00:00:00Z',
  },
];

const mockPermissions: DelegationRule[] = [
  { caller_id: 'agent-a', target_id: 'agent-b', created_at: '2024-01-01T00:00:00Z' },
];

describe('DelegationPermissions', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('displays current delegation permissions', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: mockPermissions,
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('agent-a')).toBeInTheDocument();
      expect(screen.getByText('agent-b')).toBeInTheDocument();
    });
  });

  it('shows empty state when no permissions exist', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: [],
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('No delegation permissions configured')).toBeInTheDocument();
    });
  });

  it('adds a new delegation permission', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: [],
    });
    vi.mocked(api.delegationAdd).mockResolvedValue({
      ok: true,
      message: 'Delegation permission added',
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('No delegation permissions configured')).toBeInTheDocument();
    });

    // Select caller
    const callerSelect = screen.getByLabelText('Caller agent');
    await userEvent.selectOptions(callerSelect, 'agent-a');

    // Select target
    const targetSelect = screen.getByLabelText('Target agent');
    await userEvent.selectOptions(targetSelect, 'agent-b');

    // Click Add
    const addButton = screen.getByRole('button', { name: /add/i });
    await userEvent.click(addButton);

    await waitFor(() => {
      expect(api.delegationAdd).toHaveBeenCalledWith('agent-a', 'agent-b');
    });
  });

  it('shows remove confirmation dialog', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: mockPermissions,
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('agent-a')).toBeInTheDocument();
    });

    // Click Remove button
    const removeButton = screen.getByRole('button', { name: /remove/i });
    await userEvent.click(removeButton);

    // Confirmation dialog should appear
    await waitFor(() => {
      expect(screen.getByText(/Remove Delegation Permission/)).toBeInTheDocument();
    });
  });

  it('removes a delegation permission after confirmation', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: mockPermissions,
    });
    vi.mocked(api.delegationRemove).mockResolvedValue({
      ok: true,
      message: 'Delegation permission removed',
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('agent-a')).toBeInTheDocument();
    });

    // Click Remove button in the table row
    const removeButton = screen.getByRole('button', { name: /remove/i });
    await userEvent.click(removeButton);

    // Confirm removal — the dialog has a "Remove" button with different styling
    const allRemoveButtons = screen.getAllByRole('button', { name: /^Remove$/ });
    // The confirmation dialog button is the last one (in the modal)
    const confirmButton = allRemoveButtons[allRemoveButtons.length - 1];
    await userEvent.click(confirmButton);

    await waitFor(() => {
      expect(api.delegationRemove).toHaveBeenCalledWith('agent-a', 'agent-b');
    });
  });

  it('shows cycle warning when selection would create a cycle', async () => {
    // Existing: agent-a → agent-b
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: mockPermissions,
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('agent-a')).toBeInTheDocument();
    });

    // Try to add agent-b → agent-a (would create cycle)
    const callerSelect = screen.getByLabelText('Caller agent');
    await userEvent.selectOptions(callerSelect, 'agent-b');

    const targetSelect = screen.getByLabelText('Target agent');
    await userEvent.selectOptions(targetSelect, 'agent-a');

    // Cycle warning should appear
    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
      expect(screen.getByText(/would create a circular delegation chain/)).toBeInTheDocument();
    });

    // Add button should be disabled
    const addButton = screen.getByRole('button', { name: /add/i });
    expect(addButton).toBeDisabled();
  });

  it('does not show cycle warning for valid additions', async () => {
    vi.mocked(api.delegationList).mockResolvedValue({
      ok: true,
      data: mockPermissions,
    });

    render(<DelegationPermissions agents={mockAgents} />);

    await waitFor(() => {
      expect(screen.getByText('agent-a')).toBeInTheDocument();
    });

    // Try to add agent-b → agent-c (no cycle)
    const callerSelect = screen.getByLabelText('Caller agent');
    await userEvent.selectOptions(callerSelect, 'agent-b');

    const targetSelect = screen.getByLabelText('Target agent');
    await userEvent.selectOptions(targetSelect, 'agent-c');

    // No cycle warning should appear
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();

    // Add button should be enabled
    const addButton = screen.getByRole('button', { name: /add/i });
    expect(addButton).not.toBeDisabled();
  });
});

// ── Unit tests for wouldCreateCycle ────────────────────────────────

describe('wouldCreateCycle', () => {
  it('detects self-delegation as a cycle', () => {
    expect(wouldCreateCycle([], 'a', 'a')).toBe(true);
  });

  it('detects direct cycle', () => {
    const perms: DelegationRule[] = [
      { caller_id: 'a', target_id: 'b', created_at: '' },
    ];
    expect(wouldCreateCycle(perms, 'b', 'a')).toBe(true);
  });

  it('detects transitive cycle', () => {
    const perms: DelegationRule[] = [
      { caller_id: 'a', target_id: 'b', created_at: '' },
      { caller_id: 'b', target_id: 'c', created_at: '' },
    ];
    expect(wouldCreateCycle(perms, 'c', 'a')).toBe(true);
  });

  it('allows valid non-cyclic addition', () => {
    const perms: DelegationRule[] = [
      { caller_id: 'a', target_id: 'b', created_at: '' },
    ];
    expect(wouldCreateCycle(perms, 'b', 'c')).toBe(false);
  });

  it('allows addition in disconnected graph', () => {
    const perms: DelegationRule[] = [
      { caller_id: 'a', target_id: 'b', created_at: '' },
      { caller_id: 'c', target_id: 'd', created_at: '' },
    ];
    expect(wouldCreateCycle(perms, 'd', 'a')).toBe(false);
  });
});
