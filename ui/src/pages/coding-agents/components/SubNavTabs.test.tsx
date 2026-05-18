import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router-dom';
import { describe, it, expect } from 'vitest';
import SubNavTabs from './SubNavTabs';
import type { TabItem } from './SubNavTabs';

const tabs: TabItem[] = [
  { to: '/ui/coding-agents/agent-1', label: 'Tasks' },
  { to: '/ui/coding-agents/agent-1/costs', label: 'Costs' },
  { to: '/ui/coding-agents/agent-1/new-task', label: 'New Task' },
  { to: '/ui/coding-agents/agent-1/settings', label: 'Settings' },
];

function renderWithRouter(initialPath: string) {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <SubNavTabs tabs={tabs} />
    </MemoryRouter>
  );
}

describe('SubNavTabs', () => {
  it('renders all tabs in desktop view', () => {
    renderWithRouter('/ui/coding-agents/agent-1');

    expect(screen.getByRole('navigation')).toBeInTheDocument();
    tabs.forEach((tab) => {
      expect(screen.getByRole('link', { name: tab.label })).toBeInTheDocument();
    });
  });

  it('applies active styles to the current route tab', () => {
    renderWithRouter('/ui/coding-agents/agent-1/costs');

    const costsLink = screen.getByRole('link', { name: 'Costs' });
    expect(costsLink.className).toContain('border-b-2');
    expect(costsLink.className).toContain('border-[var(--color-accent)]');
  });

  it('does not apply active styles to non-active tabs', () => {
    renderWithRouter('/ui/coding-agents/agent-1/costs');

    const tasksLink = screen.getByRole('link', { name: 'Tasks' });
    expect(tasksLink.className).not.toContain('border-b-2');
  });

  it('renders dropdown button for mobile view', () => {
    renderWithRouter('/ui/coding-agents/agent-1');

    const dropdownButton = screen.getByRole('button', { name: /tasks|select tab/i });
    expect(dropdownButton).toBeInTheDocument();
  });

  it('toggles dropdown menu on button click', async () => {
    const user = userEvent.setup();
    renderWithRouter('/ui/coding-agents/agent-1');

    const dropdownButton = screen.getByRole('button');

    // Initially dropdown items are not visible (beyond the nav links)
    const allLinks = screen.getAllByRole('link', { name: 'Costs' });
    expect(allLinks.length).toBe(1); // Only the desktop nav link

    // Click to open dropdown
    await user.click(dropdownButton);

    // Now dropdown links should be visible
    const allCostsLinks = screen.getAllByRole('link', { name: 'Costs' });
    expect(allCostsLinks.length).toBe(2); // Desktop + dropdown
  });

  it('closes dropdown when a tab is clicked', async () => {
    const user = userEvent.setup();
    renderWithRouter('/ui/coding-agents/agent-1');

    const dropdownButton = screen.getByRole('button');
    await user.click(dropdownButton);

    // Click a dropdown link
    const dropdownLinks = screen.getAllByRole('link', { name: 'Costs' });
    await user.click(dropdownLinks[1]); // Click the dropdown one

    // Dropdown should be closed - only desktop links remain
    const remainingLinks = screen.getAllByRole('link', { name: 'Costs' });
    expect(remainingLinks.length).toBe(1);
  });

  it('shows active tab label in dropdown button', () => {
    renderWithRouter('/ui/coding-agents/agent-1/costs');

    const dropdownButton = screen.getByRole('button');
    expect(dropdownButton).toHaveTextContent('Costs');
  });

  it('uses NavLink with correct to prop for each tab', () => {
    renderWithRouter('/ui/coding-agents/agent-1');

    tabs.forEach((tab) => {
      const link = screen.getByRole('link', { name: tab.label });
      expect(link).toHaveAttribute('href', tab.to);
    });
  });
});
