import { useCallback, useState } from 'react';

const RECENT_MAX = 8;
const sameValue = (a: unknown, b: unknown) => a === b;

function load<T>(storageKey: string): T[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(storageKey) ?? '[]');
    return Array.isArray(parsed) ? parsed as T[] : [];
  } catch {
    return [];
  }
}

function store<T>(storageKey: string, items: T[]): T[] {
  try {
    localStorage.setItem(storageKey, JSON.stringify(items));
  } catch {
    // Recent searches are a convenience, so a storage failure only means they aren't remembered
  }
  return items;
}

/**
 * Recent searches remembered in `localStorage` under `storageKey`, most recent first. `same` says
 * whether two entries are the same search, so saving one again moves it to the front.
 */
export function useRecentSearches<T = string>(
  storageKey: string,
  same: (a: T, b: T) => boolean = sameValue,
) {
  const [recent, setRecent] = useState<T[]>(() => load<T>(storageKey));

  const save = useCallback((item: T) => {
    setRecent(prev => store(storageKey, [item, ...prev.filter(r => !same(r, item))].slice(0, RECENT_MAX)));
  }, [storageKey, same]);

  const remove = useCallback((item: T) => {
    setRecent(prev => store(storageKey, prev.filter(r => !same(r, item))));
  }, [storageKey, same]);

  return { recent, save, remove };
}
