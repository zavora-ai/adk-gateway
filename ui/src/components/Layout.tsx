import { NavLink, Outlet } from 'react-router-dom';
import { useState } from 'react';

const navItems = [
  { to: '/ui', label: '📊 Dashboard', end: true },
  { to: '/ui/agent', label: '🤖 Model Providers' },
  { to: '/ui/agents', label: '🧩 Agents' },
  { to: '/ui/coding-agents', label: '💻 Coding Agents' },
  { to: '/ui/channels', label: '📡 Channels' },
  { to: '/ui/sessions', label: '👥 Sessions' },
  { to: '/ui/awp', label: '🌐 AWP' },
  { to: '/ui/integrations', label: '🔌 Integrations' },
  { to: '/ui/scheduled-tasks', label: '📅 Scheduled Tasks' },
  { to: '/ui/config', label: '⚙️ Configuration' },
  { to: '/ui/logs', label: '📋 Logs' },
  { to: '/ui/memory', label: '🧠 Memory' },
  { to: '/ui/settings', label: '🔧 Settings' },
];

export default function Layout() {
  const [sidebarOpen, setSidebarOpen] = useState(false);

  return (
    <div className="flex min-h-screen bg-gray-50">
      {/* Mobile hamburger */}
      <button
        className="md:hidden fixed top-3 left-3 z-50 p-2 rounded-lg bg-[var(--color-sidebar)] text-white"
        onClick={() => setSidebarOpen(!sidebarOpen)}
      >
        {sidebarOpen ? '✕' : '☰'}
      </button>

      {/* Sidebar */}
      <nav
        className={`
          fixed md:static z-40 w-56 min-h-screen bg-[var(--color-sidebar)] text-gray-400 
          flex flex-col transition-transform duration-200
          ${sidebarOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'}
        `}
      >
        <div className="px-5 py-5 border-b border-gray-700">
          <h1 className="text-white font-semibold text-base">
            <span className="text-[var(--color-accent)]">⚡</span> adk-gateway
          </h1>
        </div>

        <div className="flex-1 py-2">
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.end}
              onClick={() => setSidebarOpen(false)}
              className={({ isActive }) =>
                `block px-5 py-3 text-sm border-l-3 transition-colors ${
                  isActive
                    ? 'bg-[var(--color-sidebar-hover)] text-white border-[var(--color-accent)]'
                    : 'border-transparent hover:bg-[var(--color-sidebar-hover)] hover:text-white'
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}
        </div>

        <div className="px-5 py-4 border-t border-gray-700 text-xs text-gray-500">
          <div>adk-gateway v0.1.0</div>
        </div>
      </nav>

      {/* Overlay for mobile */}
      {sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/50 z-30 md:hidden"
          onClick={() => setSidebarOpen(false)}
        />
      )}

      {/* Main content */}
      <main className="flex-1 p-8 overflow-x-auto">
        <Outlet />
      </main>
    </div>
  );
}
