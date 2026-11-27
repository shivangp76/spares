import { expect, test } from 'bun:test';
import { formatSteps, parseSteps } from './intervalPreview';

test('steps read and written back', () => {
  expect(parseSteps('1m 10m')).toEqual([60, 600]);
  expect(parseSteps(' 30s, 1.5m 1h ')).toEqual([30, 90, 3600]);
  expect(parseSteps('')).toEqual([]);
  expect(parseSteps('10')).toBeNull();
  expect(parseSteps('1d')).toBeNull();
  expect(formatSteps([60, 600, 90, 7200])).toBe('1m 10m 90s 2h');
  expect(parseSteps(formatSteps([60, 600]))).toEqual([60, 600]);
});
