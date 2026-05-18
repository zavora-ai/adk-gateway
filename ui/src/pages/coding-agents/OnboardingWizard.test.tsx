import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import OnboardingWizard from './OnboardingWizard';

// Mock useNavigate
const mockNavigate = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual('react-router-dom');
  return {
    ...actual,
    useNavigate: () => mockNavigate,
  };
});

// Mock data is defined inline in the useApi mock factory below

// Track which fetcher is being called to return appropriate data
let lastFetcherResult: string | null = null;

// Mock api client - must be before useApi mock since useApi calls the fetcher
const mockVerifyCli = vi.fn();
vi.mock('../../api/client', () => ({
  api: {
    codingAgentBackends: () => {
      lastFetcherResult = 'backends';
      return Promise.resolve({ ok: true, data: [] });
    },
    verifyCli: (...args: unknown[]) => mockVerifyCli(...args),
    listDirectories: () => {
      lastFetcherResult = 'directories';
      return Promise.resolve({ ok: true, data: ['/home/user/projects', '/home/user/work', '/tmp/sandbox'] });
    },
  },
}));

// Mock useApi hook - calls the fetcher to determine what data to return
vi.mock('../../hooks/useApi', () => ({
  useApi: (fetcher: () => unknown) => {
    // Call the fetcher to trigger the side effect that sets lastFetcherResult
    lastFetcherResult = null;
    try { fetcher(); } catch { /* ignore */ }

    if (lastFetcherResult === 'directories') {
      return {
        data: ['/home/user/projects', '/home/user/work', '/tmp/sandbox'],
        loading: false,
        error: null,
        refetch: () => {},
      };
    }

    // Default: return backends
    return {
      data: [
        { agent_type: 'kiro', display_name: 'Kiro CLI', cli_command: 'kiro', install_check_command: 'kiro --version', auth_method: { type: 'apiKey', env_var: 'KIRO_API_KEY' }, capabilities: { file_context: true, streaming_output: true, cost_reporting: true, cancellation: true }, install_instructions: 'npm install -g @kiro/cli' },
        { agent_type: 'claude', display_name: 'Claude Code', cli_command: 'claude', install_check_command: 'claude --version', auth_method: { type: 'apiKey', env_var: 'ANTHROPIC_API_KEY' }, capabilities: { file_context: true, streaming_output: true, cost_reporting: true, cancellation: false }, install_instructions: 'npm install -g @anthropic/claude-code' },
      ],
      loading: false,
      error: null,
      refetch: () => {},
    };
  },
}));

describe('OnboardingWizard', () => {
  beforeEach(() => {
    mockNavigate.mockClear();
    mockVerifyCli.mockReset();
    mockVerifyCli.mockResolvedValue({
      ok: true,
      data: { installed: true, version: '1.2.3', path: '/usr/local/bin/kiro' },
    });
  });

  it('renders the wizard with progress indicator and step 1 backend cards', () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    expect(screen.getByText('Add Coding Agent')).toBeInTheDocument();
    expect(screen.getByText('Step 1: Select Backend')).toBeInTheDocument();
    expect(screen.getByText('Kiro CLI')).toBeInTheDocument();
    expect(screen.getByText('Claude Code')).toBeInTheDocument();
    expect(screen.getByText('kiro')).toBeInTheDocument();
    expect(screen.getByText('claude')).toBeInTheDocument();
  });

  it('advances to step 2 when a backend is selected and calls verifyCli', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('Step 2: Verify CLI')).toBeInTheDocument();
    });

    expect(mockVerifyCli).toHaveBeenCalledWith('kiro');
  });

  it('shows loading spinner while checking CLI', async () => {
    mockVerifyCli.mockReturnValue(new Promise(() => {}));

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText(/Checking CLI installation/)).toBeInTheDocument();
    });
  });

  it('shows installed version on successful CLI verification', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    expect(screen.getByText('Version: 1.2.3')).toBeInTheDocument();
    expect(screen.getByText('Path: /usr/local/bin/kiro')).toBeInTheDocument();
  });

  it('enables Next button after successful CLI verification', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    const nextButton = screen.getByText('Next');
    expect(nextButton).not.toBeDisabled();
  });

  it('disables Next button when CLI is not verified', async () => {
    mockVerifyCli.mockResolvedValue({
      ok: true,
      data: { installed: false, version: null, path: null },
    });

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI not found')).toBeInTheDocument();
    });

    const nextButton = screen.getByText('Next');
    expect(nextButton).toBeDisabled();
  });

  it('shows install instructions when CLI is not found', async () => {
    mockVerifyCli.mockResolvedValue({
      ok: true,
      data: { installed: false, version: null, path: null },
    });

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI not found')).toBeInTheDocument();
    });

    expect(screen.getByText('Install instructions:')).toBeInTheDocument();
    expect(screen.getByText('npm install -g @kiro/cli')).toBeInTheDocument();
  });

  it('shows Re-check button when CLI is not found and re-runs verification', async () => {
    mockVerifyCli
      .mockResolvedValueOnce({
        ok: true,
        data: { installed: false, version: null, path: null },
      })
      .mockResolvedValueOnce({
        ok: true,
        data: { installed: true, version: '1.0.0', path: '/usr/bin/kiro' },
      });

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI not found')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Re-check'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    expect(screen.getByText('Version: 1.0.0')).toBeInTheDocument();
  });

  it('shows error message when verification request fails', async () => {
    mockVerifyCli.mockResolvedValue({
      ok: false,
      message: 'Server error',
    });

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('Server error')).toBeInTheDocument();
    });

    expect(screen.getByText('Re-check')).toBeInTheDocument();
  });

  it('shows error message on network error', async () => {
    mockVerifyCli.mockRejectedValue(new Error('Network error'));

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('Network error')).toBeInTheDocument();
    });
  });

  it('navigates forward through steps when CLI is verified', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // Step 1 -> Step 2 via backend selection
    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    // Step 2 -> Step 3
    fireEvent.click(screen.getByText('Next'));
    expect(screen.getByText('Step 3: Authentication')).toBeInTheDocument();

    // Step 3 -> Step 4 (auth step has apiKey type, save it to advance)
    const apiKeyInput = screen.getByPlaceholderText(/enter.*api.*key/i) || screen.getByRole('textbox');
    fireEvent.change(apiKeyInput, { target: { value: 'test-key' } });
    // Click save to complete auth and auto-advance
    const saveBtn = screen.getByText(/save/i);
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText('Step 4: Workspaces')).toBeInTheDocument();
    });

    // Select a workspace to enable Next
    const checkbox = screen.getByLabelText('/home/user/projects');
    fireEvent.click(checkbox);

    // Step 4 -> Step 5
    fireEvent.click(screen.getByText('Next'));
    expect(screen.getByText('Step 5: Finalize')).toBeInTheDocument();
  });

  it('navigates backward through steps', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // Go to step 2 via backend selection
    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    // Go to step 3
    fireEvent.click(screen.getByText('Next'));
    expect(screen.getByText('Step 3: Authentication')).toBeInTheDocument();

    // Go back to step 2
    fireEvent.click(screen.getByText('Back'));
    expect(screen.getByText('Step 2: Verify CLI')).toBeInTheDocument();

    // Go back to step 1
    fireEvent.click(screen.getByText('Back'));
    expect(screen.getByText('Step 1: Select Backend')).toBeInTheDocument();
  });

  it('disables Back button on step 1', () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    const backButton = screen.getByText('Back');
    expect(backButton).toBeDisabled();
  });

  it('shows Register Agent button on final step', async () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // Navigate to step 5 via backend selection + Next clicks
    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('CLI is installed')).toBeInTheDocument();
    });

    // Step 2 -> Step 3
    fireEvent.click(screen.getByText('Next'));

    // Step 3: complete auth (apiKey type)
    const apiKeyInput = screen.getByRole('textbox');
    fireEvent.change(apiKeyInput, { target: { value: 'test-key' } });
    const saveBtn = screen.getByText(/save/i);
    fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText('Step 4: Workspaces')).toBeInTheDocument();
    });

    // Select a workspace
    const checkbox = screen.getByLabelText('/home/user/projects');
    fireEvent.click(checkbox);

    // Step 4 -> Step 5
    fireEvent.click(screen.getByText('Next'));

    expect(screen.getByText('Step 5: Finalize')).toBeInTheDocument();
    expect(screen.getByText('Register Agent')).toBeInTheDocument();
    expect(screen.queryByText('Next')).not.toBeInTheDocument();
  });

  it('cancels without confirmation when no data entered', () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    fireEvent.click(screen.getByText('Cancel'));
    expect(mockNavigate).toHaveBeenCalledWith('/ui/coding-agents');
  });

  it('shows confirmation on cancel when data has been entered', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // Select a backend to set data
    fireEvent.click(screen.getByText('Kiro CLI'));

    await waitFor(() => {
      expect(screen.getByText('Step 2: Verify CLI')).toBeInTheDocument();
    });

    // Now cancel should show confirmation since backendType is set
    fireEvent.click(screen.getByText('Cancel'));
    expect(confirmSpy).toHaveBeenCalled();
    expect(mockNavigate).not.toHaveBeenCalled();

    confirmSpy.mockRestore();
  });

  it('renders WizardProgress component', () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // WizardProgress renders step circles - check for step numbers
    expect(screen.getByText('2')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();
    expect(screen.getByText('4')).toBeInTheDocument();
    expect(screen.getByText('5')).toBeInTheDocument();
  });

  it('shows message to go back when on step 2 without backend type', () => {
    render(
      <MemoryRouter>
        <OnboardingWizard />
      </MemoryRouter>
    );

    // On step 1, Next is not disabled (isNextDisabled only checks step 2 and 4)
    // Without a backend selected, step 2 will show the fallback message
    fireEvent.click(screen.getByText('Next'));
    expect(screen.getByText('Please go back and select a backend type first.')).toBeInTheDocument();
  });
});
