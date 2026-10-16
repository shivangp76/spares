import { expect, test } from 'bun:test';
import { bucketLabel, cumulative, daysBetween, fillDays, mergeColumns, niceMax, percent, toSeries, totals } from './chartData';

test('daysBetween includes both ends and crosses months', () => {
  expect(daysBetween('2026-10-30', '2026-11-02')).toEqual(['2026-10-30', '2026-10-31', '2026-11-01', '2026-11-02']);
  expect(daysBetween('2026-10-16', '2026-10-16')).toEqual(['2026-10-16']);
  expect(daysBetween('2026-10-17', '2026-10-16')).toEqual([]);
});

test('daysBetween is not thrown off by daylight saving changes', () => {
  // Most of Europe and the US change clocks in these ranges
  expect(daysBetween('2026-03-27', '2026-03-31')).toHaveLength(5);
  expect(daysBetween('2026-10-24', '2026-11-03')).toHaveLength(11);
});

test('fillDays fills days without entries with zeros', () => {
  const days = daysBetween('2026-10-01', '2026-10-03');
  const entries = [{ date: '2026-10-02', a: 1, b: 2 }];
  expect(fillDays(days, entries, e => [e.a, e.b], 2)).toEqual([[0, 0], [1, 2], [0, 0]]);
});

test('toSeries turns columns into series', () => {
  const series = toSeries([[1, 2], [3, 4]], [
    { key: 'a', label: 'A', color: 'red' },
    { key: 'b', label: 'B', color: 'blue' },
  ]);
  expect(series.map(s => s.values)).toEqual([[1, 3], [2, 4]]);
  expect(totals(series, 2)).toEqual([3, 7]);
});

test('mergeColumns adds up columns to fit, labeling each with its span', () => {
  const series = [{ key: 'a', label: 'A', color: 'red', values: [1, 2, 3, 4, 5] }];
  const merged = mergeColumns(['d1', 'd2', 'd3', 'd4', 'd5'], series, 2);
  expect(merged.labels).toEqual(['d1 – d3', 'd4 – d5']);
  expect(merged.series[0].values).toEqual([6, 9]);
  // Nothing to merge
  expect(mergeColumns(['d1'], series, 2).labels).toEqual(['d1']);
});

test('cumulative adds up as it goes', () => {
  expect(cumulative([3, 0, 2])).toEqual([3, 3, 5]);
});

test('niceMax rounds up to 1, 2, 2.5 or 5 times a power of ten', () => {
  expect(niceMax(0)).toBe(1);
  expect(niceMax(7)).toBe(10);
  expect(niceMax(13)).toBe(20);
  expect(niceMax(200)).toBe(200);
  expect(niceMax(2126)).toBe(2500);
  expect(niceMax(0.3)).toBeCloseTo(0.5);
});

test('percent rounds, and has a dash for nothing', () => {
  expect(percent(2, 3)).toBe('67%');
  expect(percent(0, 0)).toBe('–');
});

test('bucketLabel spans the edges', () => {
  expect(bucketLabel([0, 5, 10], 1)).toBe('5–10');
  expect(bucketLabel([0, 0.25, 0.5], 0)).toBe('0–0.3');
});
