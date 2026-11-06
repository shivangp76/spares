// Whether the server could be reached by the last request to it, which `navigator.onLine` can't
// tell, for the navbar's "Offline" and the pages that need the server.
import { useSyncExternalStore } from 'react';

let reachable = typeof navigator === 'undefined' || navigator.onLine;
const listeners = new Set<() => void>();

/** Called after every request to the server, and by a sync of this device's copy. */
export function reportServerReachable(now: boolean): void {
  if (now === reachable) return;
  reachable = now;
  for (const listener of listeners) listener();
}

if (typeof window !== 'undefined') window.addEventListener('offline', () => reportServerReachable(false));

export function useServerReachable(): boolean {
  return useSyncExternalStore(
    listener => { listeners.add(listener); return () => listeners.delete(listener); },
    () => reachable,
  );
}
