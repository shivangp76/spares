import { useCallback, useEffect, useState } from 'react';
import { deleteSavedSearch, listSavedSearches, saveSearch } from '../api/client';
import type { SavedSearch } from '../types/spares';
import { errorMessage } from '../utils';

/** The searches saved on the server, which `spares saved-search` shares, and saving and deleting them. */
export function useSavedSearches() {
  const [saved, setSaved] = useState<SavedSearch[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // Offline there are just none to show
    listSavedSearches().then(setSaved).catch(() => {});
  }, []);

  const save = useCallback(async (name: string, query: string) => {
    try {
      const search = await saveSearch(name, query);
      setSaved(prev => [...prev.filter(s => s.name !== search.name), search].sort((a, b) => a.name.localeCompare(b.name)));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const remove = useCallback(async (search: SavedSearch) => {
    try {
      await deleteSavedSearch(search.name);
      setSaved(prev => prev.filter(s => s.name !== search.name));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  return { saved, save, remove, error };
}
