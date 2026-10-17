import { useEffect, useState } from 'react';
import { getSearchFields } from '../api/client';
import type { SearchField } from '../types/spares';

/** What a search field holds, as the server describes it, for explaining the values shown on a
    page, like `about('c.stability')`. Undefined until the fields are fetched. */
export function useFieldAbout(): (name: string) => string | undefined {
  const [fields, setFields] = useState<SearchField[]>([]);
  useEffect(() => {
    let cancelled = false;
    // Without them, values are shown unexplained
    getSearchFields().then(f => { if (!cancelled) setFields(f); }, () => {});
    return () => { cancelled = true; };
  }, []);
  return name => fields.find(f => f.name === name)?.about;
}
