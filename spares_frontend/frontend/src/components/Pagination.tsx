import { useState } from 'react';

/**
 * Prev and Next buttons around the page number, which can be typed over to go straight to a page,
 * going there on Enter or on leaving the box.
 */
export default function Pagination({ page, pageCount, prevPage, nextPage, goToPage }: {
  page: number;
  /** How many pages there are, or null when that isn't known, as when listing without a search. */
  pageCount: number | null;
  /** Go to the previous or next page, or are null on the first or last. */
  prevPage: (() => void) | null;
  nextPage: (() => void) | null;
  goToPage: (page: number) => void;
}) {
  // What is typed in the box, or null to show the page
  const [draft, setDraft] = useState<string | null>(null);

  function commit() {
    if (draft === null) return;
    setDraft(null);
    const n = parseInt(draft, 10);
    if (Number.isNaN(n)) return;
    const target = Math.max(1, pageCount === null ? n : Math.min(pageCount, n));
    if (target !== page) goToPage(target);
  }

  return (
    <div style={{ marginTop: 16, display: 'flex', gap: 8, alignItems: 'center' }}>
      <button disabled={!prevPage} className="touch-target-small" onClick={() => prevPage?.()}>Prev</button>
      <label style={{ fontSize: 13, display: 'flex', gap: 6, alignItems: 'center' }}>
        Page
        <input
          type="number"
          inputMode="numeric"
          min={1}
          max={pageCount ?? undefined}
          value={draft ?? page}
          onChange={e => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={e => {
            if (e.key === 'Enter') commit();
            else if (e.key === 'Escape') setDraft(null);
          }}
          onFocus={e => e.currentTarget.select()}
          aria-label="Page number"
          style={{ width: `${Math.max(String(draft ?? page).length, String(pageCount ?? page).length) + 3}ch`, fontSize: 13, padding: '2px 4px' }}
        />
        {pageCount !== null && `of ${pageCount}`}
      </label>
      <button disabled={!nextPage} className="touch-target-small" onClick={() => nextPage?.()}>Next</button>
    </div>
  );
}
