import { useCallback, useEffect, useRef } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useMediaQuery } from './useMediaQuery';

/**
 * For pages with a list beside a detail panel. On phones the open detail replaces the list
 * instead of stacking below it, and gets a history entry so the back gesture returns to the list
 * where it was scrolled to.
 *
 * `close` clears the page's selection. Returns whether to show the list, and `back`, which
 * the detail's Back and close buttons should call instead of `close`.
 */
export function useDetailView(open: boolean, close: () => void) {
  const isNarrow = useMediaQuery('(max-width: 768px)');
  const location = useLocation();
  const navigate = useNavigate();
  const inHistory = (location.state as { detail?: boolean } | null)?.detail === true;
  // Whether the current history entry is one this page pushed for the open detail
  const pushed = useRef(false);
  const listScroll = useRef(0);
  const closeRef = useRef(close);
  useEffect(() => { closeRef.current = close; });

  useEffect(() => {
    if (pushed.current && !inHistory) {
      // Left the detail's entry, by going back or by a search or page change
      pushed.current = false;
      if (open) closeRef.current();
      const y = listScroll.current;
      requestAnimationFrame(() => window.scrollTo(0, y));
    } else if (pushed.current && !open) {
      // Closed some other way, e.g. by deleting the note, so drop the entry
      navigate(-1);
    } else if (isNarrow && open && !pushed.current) {
      pushed.current = true;
      listScroll.current = window.scrollY;
      navigate({ search: location.search }, { state: { detail: true } });
      window.scrollTo(0, 0);
    }
  }, [isNarrow, open, inHistory, navigate, location.search]);

  const back = useCallback(() => {
    if (pushed.current) navigate(-1);
    else close();
  }, [navigate, close]);

  return { isNarrow, showList: !(isNarrow && open), back };
}
