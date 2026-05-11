import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';

interface UseAuthResult {
  authenticated: boolean | null; // null = still checking
  mode: string;
  login: (password: string) => Promise<{ ok: boolean; message?: string }>;
  logout: () => Promise<void>;
}

/** Auth state hook — checks session on mount, provides login/logout. */
export function useAuth(): UseAuthResult {
  const [authenticated, setAuthenticated] = useState<boolean | null>(null);
  const [mode, setMode] = useState('none');

  useEffect(() => {
    api.checkAuth().then((res) => {
      if (res.ok && res.data) {
        setAuthenticated(res.data.authenticated);
        setMode(res.data.mode);
      } else {
        // If the endpoint doesn't exist yet, assume no auth required
        setAuthenticated(true);
        setMode('none');
      }
    }).catch(() => {
      setAuthenticated(true);
      setMode('none');
    });
  }, []);

  const login = useCallback(async (password: string) => {
    const res = await api.login(password);
    if (res.ok) {
      setAuthenticated(true);
    }
    return { ok: res.ok, message: res.message };
  }, []);

  const logout = useCallback(async () => {
    await api.logout();
    setAuthenticated(false);
  }, []);

  return { authenticated, mode, login, logout };
}
