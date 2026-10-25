import { useSearchParams } from 'react-router-dom';
import type { SearchSort } from '../types/spares';
import { withPage } from '../utils';

/**
 * The column a table is sorted by, kept in the URL (`?sort=c.due&dir=desc`) like the search, and
 * a way to cycle a column through ascending, descending and the query's own order.
 */
export function useTableSort(): { sort: SearchSort | null; cycle: (field: string) => void } {
  const [searchParams, setSearchParams] = useSearchParams();
  const field = searchParams.get('sort');
  const sort = field ? { field, descending: searchParams.get('dir') === 'desc' } : null;
  function cycle(next: string) {
    setSearchParams(prev => {
      const params = withPage(prev, null);
      // Sorting by a column replaces the queue order, which has no columns
      params.delete('order');
      if (sort?.field !== next) {
        params.set('sort', next);
        params.delete('dir');
      } else if (!sort.descending) {
        params.set('dir', 'desc');
      } else {
        params.delete('sort');
        params.delete('dir');
      }
      return params;
    });
  }
  return { sort, cycle };
}
