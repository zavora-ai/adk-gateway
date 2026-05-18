import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import AgentListView from './AgentListView';
import type { CodingAgentSummary } from '../../types';

// Mock useApi hook
vi.mock('../../hooks/useApi', () => ({
  useApi: vi.fn(),
}));

// Mock useWebSocket hook
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ lastEvent: null, isConnected: true })),
}));

// Mock useNavigate
const mockNavigate = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual('react-router-dom');
  return {
    ...actual,
    useNavigate: () => mockNavigate,
  };
});

import { useApi } from '../../hooks/useApi';

const mockAgents: CodingAgentSummary[] = [
  {
    id: 'agent-1',
    alias: 'My Kiro Agent',
    backend_type: 'kiro',
    display_name: 'Kiro CLI',
    connection_status: 'connected',
    last_task_at: '2026-05-14T10:30:00Z',
    workspaces: ['/home/user/project'],
  },
  {
    id: 'agent-2',
    alias: null,
    backend_type: 'claude',
    display_name: 'Claude Code',
    connection_status: 'disconnected',
    last_task_at: null,
    workspaces: [],
  },
];

describe('AgentListView', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders loading skeleton while fetching', () => {
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: null,
      loading: true,
      error: null,
      refetch: vi.fn(),
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    expect(screen.getByText('Coding Agents')).toBeInTheDocument();
    // Skeleton cards should be rendered (animate-pulse elements)
    const skeletons = document.querySelectorAll('.animate-pulse');
    expect(skeletons.length).toBe(3);
  });

  it('renders error banner with retry on failure', () => {
    const mockRefetch = vi.fn();
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: null,
      loading: false,
      error: 'Network error',
      refetch: mockRefetch,
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    expect(screen.getByText(/Failed to load agents: Network error/)).toBeInTheDocument();
    const retryButton = screen.getByText('Retry');
    expect(retryButton).toBeInTheDocument();
    retryButton.click();
    expect(mockRefetch).toHaveBeenCalledTimes(1);
  });

  it('renders empty state with Add Agent prompt when no agents', () => {
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: [],
      loading: false,
      error: null,
      refetch: vi.fn(),
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    expect(screen.getByText('No coding agents registered yet.')).toBeInTheDocument();
    const addButton = screen.getByText('Add Agent');
    expect(addButton).toBeInTheDocument();
    addButton.click();
    expect(mockNavigate).toHaveBeenCalledWith('/ui/coding-agents/new');
  });

  it('renders agent cards with correct data', () => {
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: mockAgents,
      loading: false,
      error: null,
      refetch: vi.fn(),
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    // Agent with alias shows alias
    expect(screen.getByText('My Kiro Agent')).toBeInTheDocument();
    expect(screen.getByText('Kiro CLI')).toBeInTheDocument();
    expect(screen.getByText('Connected')).toBeInTheDocument();

    // Agent without alias shows id
    expect(screen.getByText('agent-2')).toBeInTheDocument();
    expect(screen.getByText('Claude Code')).toBeInTheDocument();
    expect(screen.getByText('Disconnected')).toBeInTheDocument();
    expect(screen.getByText('No tasks yet')).toBeInTheDocument();
  });

  it('navigates to agent detail on card click', () => {
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: mockAgents,
      loading: false,
      error: null,
      refetch: vi.fn(),
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    screen.getByText('My Kiro Agent').closest('button')!.click();
    expect(mockNavigate).toHaveBeenCalledWith('/ui/coding-agents/agent-1');
  });

  it('renders Add Agent button in header when agents exist', () => {
    (useApi as ReturnType<typeof vi.fn>).mockReturnValue({
      data: mockAgents,
      loading: false,
      error: null,
      refetch: vi.fn(),
    });

    render(
      <MemoryRouter>
        <AgentListView />
      </MemoryRouter>
    );

    const addButton = screen.getByText('Add Agent');
    expect(addButton).toBeInTheDocument();
    addButton.click();
    expect(mockNavigate).toHaveBeenCalledWith('/ui/coding-agents/new');
  });
});
