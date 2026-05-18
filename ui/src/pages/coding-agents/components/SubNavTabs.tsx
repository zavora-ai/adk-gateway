import { NavLink, useLocation } from 'react-router-dom';
import { useState } from 'react';

export interface TabItem {
  to: string;
  label: string;
}

interface SubNavTabsProps {
  tabs: TabItem[];
}

export default function SubNavTabs({ tabs }: SubNavTabsProps) {
  const location = useLocation();
  const [dropdownOpen, setDropdownOpen] = useState(false);

  // Find the most specific matching tab (longest path match)
  const currentPath = location.pathname.replace(/\/$/, '');
  const activeTab = [...tabs]
    .filter((tab) => {
      const tabPath = tab.to.replace(/\/$/, '');
      return currentPath === tabPath || currentPath.startsWith(tabPath + '/');
    })
    .sort((a, b) => b.to.length - a.to.length)[0] ?? tabs[0];

  return (
    <>
      {/* Desktop tabs (>=640px) */}
      <nav className="hidden sm:flex border-b border-gray-200">
        {tabs.map((tab) => (
          <NavLink
            key={tab.to}
            to={tab.to}
            end
            className={({ isActive }) =>
              `px-4 py-2 text-sm font-medium transition-colors ${
                isActive
                  ? 'border-b-2 border-[var(--color-accent)] text-[var(--color-accent)]'
                  : 'text-gray-500 hover:text-gray-700'
              }`
            }
          >
            {tab.label}
          </NavLink>
        ))}
      </nav>

      {/* Mobile dropdown (<640px) */}
      <div className="sm:hidden relative">
        <button
          type="button"
          onClick={() => setDropdownOpen(!dropdownOpen)}
          className="w-full flex items-center justify-between px-4 py-2 text-sm font-medium text-gray-700 border border-gray-300 rounded-lg bg-white"
          aria-expanded={dropdownOpen}
          aria-haspopup="listbox"
        >
          <span>{activeTab?.label ?? 'Select tab'}</span>
          <svg
            className={`w-4 h-4 ml-2 transition-transform ${dropdownOpen ? 'rotate-180' : ''}`}
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
          </svg>
        </button>

        {dropdownOpen && (
          <div className="absolute z-10 mt-1 w-full bg-white border border-gray-200 rounded-lg shadow-lg">
            {tabs.map((tab) => (
              <NavLink
                key={tab.to}
                to={tab.to}
                end
                onClick={() => setDropdownOpen(false)}
                className={({ isActive }) =>
                  `block px-4 py-2 text-sm ${
                    isActive
                      ? 'bg-gray-50 text-[var(--color-accent)] font-medium'
                      : 'text-gray-700 hover:bg-gray-50'
                  }`
                }
              >
                {tab.label}
              </NavLink>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
