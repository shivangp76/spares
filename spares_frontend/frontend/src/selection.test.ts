import { expect, test } from 'bun:test';
import { ALL_SELECTED, NO_SELECTION, initialSelection, invertSelection, isSelected, rangeIds, resolveSelection, selectedCount, setSelected } from './selection';

test('a search starts with all of it selected', () => {
  expect(initialSelection('tag=a')).toEqual(ALL_SELECTED);
  expect(initialSelection(null)).toEqual(NO_SELECTION);
});

test('checking and unchecking rows', () => {
  const some = setSelected(NO_SELECTION, [1, 2, 3], true);
  expect(isSelected(some, 2)).toBe(true);
  expect(isSelected(some, 4)).toBe(false);
  const fewer = setSelected(some, [2], false);
  expect([...fewer.ids]).toEqual([1, 3]);
  expect(selectedCount(fewer, 100)).toBe(2);
});

test('unchecking rows of a whole search leaves them out', () => {
  const allBut = setSelected(ALL_SELECTED, [5, 6], false);
  expect(isSelected(allBut, 5)).toBe(false);
  expect(isSelected(allBut, 7)).toBe(true);
  expect(selectedCount(allBut, 10)).toBe(8);
  // Checking a left out row again puts it back
  expect(setSelected(allBut, [5], true).ids).toEqual(new Set([6]));
});

test('inverting a search, or the rows of a page', () => {
  const allBut = setSelected(ALL_SELECTED, [5], false);
  const inverted = invertSelection(allBut, null);
  expect(isSelected(inverted, 5)).toBe(true);
  expect(isSelected(inverted, 6)).toBe(false);
  expect(selectedCount(inverted, 10)).toBe(1);
  const page = invertSelection(setSelected(NO_SELECTION, [1, 9], true), [1, 2, 3]);
  expect([...page.ids].sort()).toEqual([2, 3, 9]);
});

test('shift-click ranges go either way', () => {
  const rows = [10, 11, 12, 13].map(id => ({ id }));
  expect(rangeIds(rows, 1, 3)).toEqual([11, 12, 13]);
  expect(rangeIds(rows, 2, 0)).toEqual([10, 11, 12]);
});

test('resolving a selection', async () => {
  const search = async () => [1, 2, 3, 4];
  expect(await resolveSelection(setSelected(NO_SELECTION, [2, 3], true), 'q', search)).toEqual({ Ids: [2, 3] });
  expect(await resolveSelection(ALL_SELECTED, 'q', search)).toEqual({ Query: 'q' });
  expect(await resolveSelection(setSelected(ALL_SELECTED, [2], false), 'q', search)).toEqual({ Ids: [1, 3, 4] });
  await expect(resolveSelection(ALL_SELECTED, null, search)).rejects.toThrow();
});
