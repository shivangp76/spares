import { expect, test } from 'bun:test';
import { retentionTitle, retentionTone, weakestFirst } from './tagRetention';

const r = (name: string, week: number) => ({ name, card_count: 3, today: week + 0.05, week });

test('weakest first, with tags without reviewed cards last', () => {
  const retention = new Map([r('a', 0.9), r('b', 0.6), r('c', 0.75)].map(x => [x.name, x]));
  const sorted = weakestFirst([{ name: 'a' }, { name: 'z' }, { name: 'b' }, { name: 'c' }, { name: 'y' }], retention);
  expect(sorted.map(t => t.name)).toEqual(['b', 'c', 'a', 'y', 'z']);
});

test('colours and the tooltip', () => {
  expect(retentionTone(0.9)).toBe('green');
  expect(retentionTone(0.8)).toBe('orange');
  expect(retentionTone(0.5)).toBe('red');
  expect(retentionTitle(r('a', 0.8))).toContain('85% today, 80% in 7 days');
});
