import { describe, expect, test } from 'bun:test';
import { dayAfter, localDateString } from './useBuryUntilDate';

describe('dayAfter', () => {
  test('is the start of the next local day', () => {
    expect(dayAfter('2026-11-04')).toEqual(new Date(2026, 10, 5));
  });

  test('rolls over the end of a month and year', () => {
    expect(dayAfter('2026-12-31')).toEqual(new Date(2027, 0, 1));
  });
});

describe('localDateString', () => {
  test('pads months and days, as an <input type="date"> does', () => {
    expect(localDateString(new Date(2026, 0, 5, 23, 59))).toBe('2026-01-05');
  });
});
