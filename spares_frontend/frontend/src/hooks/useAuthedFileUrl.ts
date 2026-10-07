import { useEffect, useState } from 'react';
import { fetchFile } from '../api/client';

interface Loaded { key: string; url: string | null; error: string | null }

/**
 * An object URL for a rendered file, for elements like `<embed>` that cannot send the API key.
 * Changing `version` refetches the file, e.g. after it was regenerated.
 */
export function useAuthedFileUrl(path: string, version?: number): { url: string | null; error: string | null } {
  const key = `${path}?v=${version}`;
  const [loaded, setLoaded] = useState<Loaded | null>(null);

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | null = null;
    fetchFile(path)
      .then(blob => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setLoaded({ key, url: objectUrl, error: null });
      })
      .catch(e => { if (!cancelled) setLoaded({ key, url: null, error: String(e) }); });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [key, path]);

  // A file still loading for a new path or version shows nothing rather than the previous one
  return loaded?.key === key ? loaded : { url: null, error: null };
}
