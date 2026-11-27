import { useMemo } from "react";
import { getCredentials, logout as endSession } from "../api/client";
import type { Credentials } from "../types/spares";
import { appPath } from "../demo";

export function useAuth(): {
  credentials: Credentials | null;
  logout: () => void;
} {
  const credentials = useMemo<Credentials | null>(() => {
    try {
      return getCredentials();
    } catch {
      return null;
    }
  }, []);

  function logout() {
    void endSession().then(() => window.location.replace(appPath("/login")));
  }

  return { credentials, logout };
}
