import { useEffect, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';

/**
 * Wraps the Dashboard route to detect first-run state.
 * If no primary model is configured and setup hasn't been completed,
 * redirects to the setup wizard.
 */
export default function SetupRedirect({ children }: { children: ReactNode }) {
  const navigate = useNavigate();
  const [checked, setChecked] = useState(false);

  useEffect(() => {
    // Skip if setup was already completed
    if (localStorage.getItem('adk_setup_complete') === 'true') {
      setChecked(true);
      return;
    }

    // Check if primary model is configured
    let cancelled = false;
    (async () => {
      try {
        const res = await api.getAgent();
        if (cancelled) return;

        if (res.ok && res.data && res.data.primary) {
          // Model is configured — mark setup as done
          localStorage.setItem('adk_setup_complete', 'true');
          setChecked(true);
        } else {
          // No model configured — redirect to setup
          navigate('/ui/setup', { replace: true });
        }
      } catch {
        // If API fails (fresh install), redirect to setup
        if (!cancelled) {
          navigate('/ui/setup', { replace: true });
        }
      }
    })();

    return () => { cancelled = true; };
  }, [navigate]);

  if (!checked) {
    return (
      <div className="flex items-center justify-center py-20">
        <div className="text-gray-400 text-sm">Checking configuration...</div>
      </div>
    );
  }

  return <>{children}</>;
}
