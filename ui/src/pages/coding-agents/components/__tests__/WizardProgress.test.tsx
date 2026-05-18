/**
 * Unit tests for WizardProgress component — Task 2.3
 *
 * Tests verify:
 * - Renders correct number of step circles
 * - Completed steps show accent fill
 * - Current step shows ring/border
 * - Remaining steps show gray styling
 * - Connector lines between steps use correct colors
 *
 * @vitest-environment jsdom
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import WizardProgress from '../WizardProgress';

describe('WizardProgress', () => {
  it('renders the correct number of step circles', () => {
    const { container } = render(<WizardProgress currentStep={1} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');
    expect(circles).toHaveLength(5);
  });

  it('marks steps before currentStep as completed with accent background', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');

    // Steps 1 and 2 should be completed (accent fill)
    expect(circles[0].className).toContain('bg-[var(--color-accent)]');
    expect(circles[0].className).toContain('text-white');
    expect(circles[1].className).toContain('bg-[var(--color-accent)]');
    expect(circles[1].className).toContain('text-white');
  });

  it('marks the current step with a border ring', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');

    // Step 3 should be current (border ring)
    expect(circles[2].className).toContain('border-2');
    expect(circles[2].className).toContain('border-[var(--color-accent)]');
  });

  it('marks steps after currentStep as remaining with gray styling', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');

    // Steps 4 and 5 should be remaining (gray)
    expect(circles[3].className).toContain('bg-gray-200');
    expect(circles[3].className).toContain('text-gray-500');
    expect(circles[4].className).toContain('bg-gray-200');
    expect(circles[4].className).toContain('text-gray-500');
  });

  it('shows checkmark icon for completed steps', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');

    // Completed steps should have an SVG checkmark
    expect(circles[0].querySelector('svg')).not.toBeNull();
    expect(circles[1].querySelector('svg')).not.toBeNull();
  });

  it('shows step number for current and remaining steps', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const circles = container.querySelectorAll('.rounded-full');

    // Current step shows its number
    expect(circles[2].textContent).toBe('3');
    // Remaining steps show their numbers
    expect(circles[3].textContent).toBe('4');
    expect(circles[4].textContent).toBe('5');
  });

  it('renders connector lines between steps with correct colors', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={5} />);
    const lines = container.querySelectorAll('.h-0\\.5');

    // Should have 4 connector lines (totalSteps - 1)
    expect(lines).toHaveLength(4);

    // Lines before current step should be accent colored
    expect(lines[0].className).toContain('bg-[var(--color-accent)]');
    expect(lines[1].className).toContain('bg-[var(--color-accent)]');

    // Lines at and after current step should be gray
    expect(lines[2].className).toContain('bg-gray-200');
    expect(lines[3].className).toContain('bg-gray-200');
  });

  it('handles first step correctly', () => {
    const { container } = render(<WizardProgress currentStep={1} totalSteps={3} />);
    const circles = container.querySelectorAll('.rounded-full');

    // First step is current
    expect(circles[0].className).toContain('border-2');
    // Others are remaining
    expect(circles[1].className).toContain('bg-gray-200');
    expect(circles[2].className).toContain('bg-gray-200');
  });

  it('handles last step correctly', () => {
    const { container } = render(<WizardProgress currentStep={3} totalSteps={3} />);
    const circles = container.querySelectorAll('.rounded-full');

    // First two are completed
    expect(circles[0].className).toContain('bg-[var(--color-accent)]');
    expect(circles[1].className).toContain('bg-[var(--color-accent)]');
    // Last is current
    expect(circles[2].className).toContain('border-2');
  });
});
