import { useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { clearCredentials, getCredentials } from '../api/client';
import type { Credentials } from '../types/spares';

export function useAuth(): { credentials: Credentials | null; logout: () => void } {
  const navigate = useNavigate();
  const credentials = useMemo<Credentials | null>(() => {
    try {
      return getCredentials();
    } catch {
      return null;
    }
  }, []);

  function logout() {
    clearCredentials();
    navigate('/login');
  }

  return { credentials, logout };
}
