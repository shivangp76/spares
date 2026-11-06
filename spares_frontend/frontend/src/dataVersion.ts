import { useSyncExternalStore } from "react";
import { clearCaches } from "./api/client";

// Bumped by `dataChanged`, so pages that depend on it fetch what they show again
let version = 0;
const listeners = new Set<() => void>();

/**
 * Tells the pages that something they show may have changed without them knowing what, as an undo,
 * redo or sync can change anything. Forgets the responses kept in memory, and pages using
 * `useDataVersion` fetch again, rather than the whole app being reloaded.
 */
export function dataChanged(): void {
  clearCaches();
  version += 1;
  for (const listener of listeners) listener();
}

/** Calls `listener` after each `dataChanged`. Returns a function that stops it. */
export function onDataChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * A number that changes after each `dataChanged`. A page puts it in the dependencies of the effects
 * that fetch what it shows, so they fetch again.
 */
export function useDataVersion(): number {
  return useSyncExternalStore(onDataChanged, () => version);
}
