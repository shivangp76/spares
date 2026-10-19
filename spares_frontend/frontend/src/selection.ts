/**
 * Which rows of a list a bulk action acts on. A search starts with every row it finds selected, so
 * that it can be acted on as a whole, however many pages it has.
 */
export interface RowSelection {
  /** Whether every row the search finds is selected, except `ids`. Otherwise just `ids` are. */
  all: boolean;
  ids: ReadonlySet<number>;
}

export const NO_SELECTION: RowSelection = { all: false, ids: new Set() };

export const ALL_SELECTED: RowSelection = { all: true, ids: new Set() };

/** What a bulk action acts on, as `NotesSelector` and `CardsSelector` take it. */
export type IdsOrQuery = { Ids: number[] } | { Query: string };

/** What is selected when `query` is searched for, or the search is cleared if `null`. */
export function initialSelection(query: string | null): RowSelection {
  return query === null ? NO_SELECTION : ALL_SELECTED;
}

export function isSelected(selection: RowSelection, id: number): boolean {
  return selection.all !== selection.ids.has(id);
}

/** `selection` with `ids` checked, or unchecked if `!select`. */
export function setSelected(selection: RowSelection, ids: number[], select: boolean): RowSelection {
  const next = new Set(selection.ids);
  // In `all`, `ids` are the rows left out
  const add = select !== selection.all;
  for (const id of ids) {
    if (add) next.add(id);
    else next.delete(id);
  }
  return { all: selection.all, ids: next };
}

/**
 * `selection` with what is checked unchecked, and the other way round: of the whole search, or
 * without one, of the rows with `ids`.
 */
export function invertSelection(selection: RowSelection, ids: number[] | null): RowSelection {
  if (ids === null) return { all: !selection.all, ids: selection.ids };
  const next = new Set(selection.ids);
  for (const id of ids) {
    if (next.has(id)) next.delete(id);
    else next.add(id);
  }
  return { all: selection.all, ids: next };
}

/** How many rows `selection` has, given that the search finds `total`. */
export function selectedCount(selection: RowSelection, total: number): number {
  return selection.all ? total - selection.ids.size : selection.ids.size;
}

/** The ids of `rows` from index `from` to `to`, either way round, for a shift-click. */
export function rangeIds(rows: { id: number }[], from: number, to: number): number[] {
  return rows.slice(Math.min(from, to), Math.max(from, to) + 1).map(r => r.id);
}

/**
 * What `selection` acts on. A whole search is sent as its query; leaving rows out of it needs its
 * ids, which `search` finds.
 */
export async function resolveSelection(
  selection: RowSelection,
  query: string | null,
  search: (query: string) => Promise<number[]>,
): Promise<IdsOrQuery> {
  if (!selection.all) return { Ids: [...selection.ids] };
  if (query === null) throw new Error('Every row is selected, but there is no search');
  if (selection.ids.size === 0) return { Query: query };
  return { Ids: (await search(query)).filter(id => !selection.ids.has(id)) };
}
