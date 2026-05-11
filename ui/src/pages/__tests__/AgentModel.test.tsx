/**
 * UI Tests for AgentModel page — Task 7.6
 *
 * Tests verify:
 * - Cloud provider form persistence round-trip (Task 7.6)
 * - Cloud provider validation behavior (Task 7.6)
 * - Cloud config included in save payload (Task 7.6)
 *
 * @vitest-environment jsdom
 *
 * Prerequisites:
 *   npm install -D vitest @testing-library/react @testing-library/jest-dom jsdom
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import AgentModel from '../AgentModel';

// ── Mocks ──────────────────────────────────────────────────────────

const mockSaveAgent = vi.fn();

vi.mock('../../api/client', () => ({
  api: {
    getAgent: vi.fn().mockResolvedValue({
      ok: true,
      data: {
        primary: 'gpt-4o',
        cloud_provider: {
          type: 'azure',
          endpoint_url: 'https://my-resource.openai.azure.com/',
          api_version: '2024-02-01',
          deployment_name: 'gpt-4o-deploy',
        },
      },
    }),
    saveAgent: (...args: unknown[]) => mockSaveAgent(...args),
  },
}));

vi.mock('../../api/modelProviders', () => ({
  extractProvider: (modelId: string) => {
    const parts = modelId.split('/');
    return parts.length > 1 ? parts[0] : null;
  },
}));

vi.mock('../../components/ModelSelector', () => ({
  default: () => <div data-testid="model-selector" />,
}));

// ── Task 7.6: Cloud Provider Form Persistence ──────────────────────

describe('Cloud Provider Form Persistence', () => {
  beforeEach(() => {
    mockSaveAgent.mockClear();
    mockSaveAgent.mockResolvedValue({ ok: true, message: 'Saved.' });
  });

  it('populates cloud config fields from API response on load', async () => {
    render(<AgentModel />);

    await waitFor(() => {
      // Should show the enterprise cloud provider section with pre-filled values
      const endpointInput = screen.getByDisplayValue('https://my-resource.openai.azure.com/');
      expect(endpointInput).toBeInTheDocument();
    });

    expect(screen.getByDisplayValue('2024-02-01')).toBeInTheDocument();
    expect(screen.getByDisplayValue('gpt-4o-deploy')).toBeInTheDocument();
  });

  it('includes cloud_provider in save payload', async () => {
    render(<AgentModel />);

    await waitFor(() => {
      expect(screen.getByDisplayValue('https://my-resource.openai.azure.com/')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      expect(mockSaveAgent).toHaveBeenCalledWith(
        expect.objectContaining({
          cloud_provider: expect.objectContaining({
            type: 'azure',
            endpoint_url: 'https://my-resource.openai.azure.com/',
            api_version: '2024-02-01',
            deployment_name: 'gpt-4o-deploy',
          }),
        }),
      );
    });
  });

  it('shows validation errors when required fields are empty', async () => {
    const { api } = await import('../../api/client');
    (api.getAgent as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ok: true,
      data: {
        primary: null,
        cloud_provider: { type: 'azure' },
      },
    });

    render(<AgentModel />);

    await waitFor(() => {
      expect(screen.getByText('Save Configuration')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      expect(screen.getByText('Endpoint URL is required')).toBeInTheDocument();
      expect(screen.getByText('API Version is required')).toBeInTheDocument();
      expect(screen.getByText('Deployment Name is required')).toBeInTheDocument();
    });

    // Save should not have been called
    expect(mockSaveAgent).not.toHaveBeenCalled();
  });

  it('switches cloud provider and shows correct fields', async () => {
    render(<AgentModel />);

    await waitFor(() => {
      expect(screen.getByText('Google Vertex AI')).toBeInTheDocument();
    });

    // Click GCP provider
    fireEvent.click(screen.getByText('Google Vertex AI'));

    await waitFor(() => {
      expect(screen.getByText('Project ID')).toBeInTheDocument();
      expect(screen.getByText('Region')).toBeInTheDocument();
      expect(screen.getByText('Model Name')).toBeInTheDocument();
    });
  });

  it('validates GCP fields when GCP is selected', async () => {
    const { api } = await import('../../api/client');
    (api.getAgent as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ok: true,
      data: {
        primary: null,
        cloud_provider: { type: 'gcp' },
      },
    });

    render(<AgentModel />);

    await waitFor(() => {
      expect(screen.getByText('Save Configuration')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      expect(screen.getByText('Project ID is required')).toBeInTheDocument();
      expect(screen.getByText('Region is required')).toBeInTheDocument();
      expect(screen.getByText('Model Name is required')).toBeInTheDocument();
    });
  });

  it('validates Bedrock fields when Bedrock is selected', async () => {
    const { api } = await import('../../api/client');
    (api.getAgent as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      ok: true,
      data: {
        primary: null,
        cloud_provider: { type: 'bedrock' },
      },
    });

    render(<AgentModel />);

    await waitFor(() => {
      expect(screen.getByText('Save Configuration')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Save Configuration'));

    await waitFor(() => {
      expect(screen.getByText('Region is required')).toBeInTheDocument();
      expect(screen.getByText('AWS Profile is required')).toBeInTheDocument();
      expect(screen.getByText('Model ARN is required')).toBeInTheDocument();
    });
  });
});
