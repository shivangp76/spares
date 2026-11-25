// Whether the server could be reached by the last request to it, which `navigator.onLine` can't
// tell, for the navbar's "Offline" and the pages that need the server. While it can't be reached,
// the server is asked every few seconds whether it is back, so pages can fetch again by themselves.
import { useSyncExternalStore } from "react";

let reachable = typeof navigator === "undefined" || navigator.onLine;
const listeners = new Set<() => void>();
const backListeners = new Set<() => void>();

// How often to ask whether the server is back
const PROBE_INTERVAL_MS = 5000;
let probeTimer: ReturnType<typeof setInterval> | null = null;

/** Called after every request to the server, and by a sync of this device's copy. */
export function reportServerReachable(now: boolean): void {
  if (now === reachable) return;
  reachable = now;
  if (now) stopProbing();
  else startProbing();
  for (const listener of listeners) listener();
  if (now) for (const listener of backListeners) listener();
}

/** Calls `listener` each time the server can be reached again. Returns a function that stops it. */
export function onServerBack(listener: () => void): () => void {
  backListeners.add(listener);
  return () => backListeners.delete(listener);
}

/**
 * Whether `res` came from a proxy in front of the server rather than the server, which it
 * couldn't reach, like the dev server's 502 when the server isn't running. The server's own
 * errors are JSON.
 */
export function isGatewayError(res: Response): boolean {
  return (
    [502, 503, 504].includes(res.status) &&
    !(res.headers.get("Content-Type") ?? "").includes("json")
  );
}

/** Asks the server whether it is back, through the cheapest request it answers without a session. */
async function probe(): Promise<void> {
  if (document.visibilityState !== "visible") return;
  try {
    const res = await fetch(`${window.location.origin}/api/version`, {
      cache: "no-store",
    });
    if (res.ok) reportServerReachable(true);
  } catch {
    // Still offline
  }
}

function startProbing(): void {
  if (typeof window === "undefined" || probeTimer !== null) return;
  probeTimer = setInterval(() => void probe(), PROBE_INTERVAL_MS);
}

function stopProbing(): void {
  if (probeTimer === null) return;
  clearInterval(probeTimer);
  probeTimer = null;
}

if (typeof window !== "undefined") {
  window.addEventListener("offline", () => reportServerReachable(false));
  window.addEventListener("online", () => {
    if (!reachable) void probe();
  });
  document.addEventListener("visibilitychange", () => {
    if (!reachable) void probe();
  });
  if (!reachable) startProbing();
}

export function useServerReachable(): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => reachable,
  );
}
