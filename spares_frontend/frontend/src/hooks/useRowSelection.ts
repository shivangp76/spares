import { useRef, useState } from 'react';
import { ALL_SELECTED, NO_SELECTION, initialSelection, isSelected, rangeIds, setSelected, type RowSelection } from '../selection';

/**
 * The rows of a list checked for a bulk action, kept across its pages and started again by each
 * new search.
 */
export function useRowSelection(query: string | null) {
  const [selection, setSelection] = useState(() => initialSelection(query));
  const [selectionQuery, setSelectionQuery] = useState(query);
  if (selectionQuery !== query) {
    setSelectionQuery(query);
    setSelection(initialSelection(query));
  }
  // The row last checked or unchecked on this page, which a shift-click extends from
  const anchor = useRef<number | null>(null);

  /** Checks or unchecks the `index`th of `rows`, and with `shift`, the rows back to the last one clicked. */
  function toggleRow(rows: { id: number }[], index: number, shift: boolean) {
    const select = !isSelected(selection, rows[index].id);
    const ids = shift && anchor.current !== null && anchor.current < rows.length
      ? rangeIds(rows, anchor.current, index)
      : [rows[index].id];
    anchor.current = index;
    setSelection(prev => setSelected(prev, ids, select));
  }

  /** The header checkbox: the whole search, or without one, the rows of this page. */
  function header(rows: { id: number }[]): { checked: boolean; indeterminate: boolean; toggle: () => void } {
    const onPage = rows.filter(r => isSelected(selection, r.id)).length;
    if (query !== null) {
      const whole = selection.all && selection.ids.size === 0;
      return {
        checked: whole,
        indeterminate: !whole && (selection.all || selection.ids.size > 0),
        toggle: () => setSelection(whole ? NO_SELECTION : ALL_SELECTED),
      };
    }
    const allOnPage = rows.length > 0 && onPage === rows.length;
    return {
      checked: allOnPage,
      indeterminate: !allOnPage && onPage > 0,
      toggle: () => setSelection(prev => setSelected(prev, rows.map(r => r.id), !allOnPage)),
    };
  }

  return {
    selection,
    setSelection: (next: RowSelection) => setSelection(next),
    /** Starts again as if the search had just been made, e.g. after an action changed what it finds. */
    reset: () => { anchor.current = null; setSelection(initialSelection(query)); },
    isSelected: (id: number) => isSelected(selection, id),
    toggleRow,
    header,
  };
}
