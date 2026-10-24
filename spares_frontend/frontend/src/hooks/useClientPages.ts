import { useState } from 'react';
import type { PageSize } from '../preferences';

/**
 * Splits rows that are all loaded at once, as tags and keywords are, into pages, giving the rows of
 * the page shown and what `Pagination` takes. The page is kept within the pages there are, so it
 * stays on the last when rows are removed from it.
 */
export function useClientPages<T>(rows: T[], pageSizeChoice: PageSize, setPageSizeChoice: (size: PageSize) => void) {
  const [chosenPage, setChosenPage] = useState(1);
  const pageSize = Number(pageSizeChoice);
  const pageCount = Math.max(1, Math.ceil(rows.length / pageSize));
  const page = Math.min(chosenPage, pageCount);
  const offset = (page - 1) * pageSize;

  const goToPage = (n: number) => setChosenPage(n);
  /** Lists `size` rows on each page, from the page with the first row of this one. */
  function setPageSize(size: PageSize) {
    setChosenPage(Math.floor(offset / Number(size)) + 1);
    setPageSizeChoice(size);
  }

  return {
    /** The rows on the page shown. */
    shown: rows.slice(offset, offset + pageSize),
    /** Where the page shown starts in `rows`. */
    offset,
    pagination: {
      page,
      pageCount,
      prevPage: page > 1 ? () => goToPage(page - 1) : null,
      nextPage: page < pageCount ? () => goToPage(page + 1) : null,
      goToPage,
      pageSize: pageSizeChoice,
      setPageSize,
    },
  };
}
