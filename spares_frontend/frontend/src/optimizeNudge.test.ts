import { expect, test } from 'bun:test';
import { sinceOptimized, stillDismissed } from './optimizeNudge';
import type { OptimizeStatusResponse } from './types/spares';

const status = (more: Partial<OptimizeStatusResponse>): OptimizeStatusResponse => ({
  optimizable: true,
  review_count: 2000,
  fitted_at: '2026-10-01',
  reviews_since: 512,
  days_since: 10,
  suggested: true,
  after_reviews: 500,
  after_days: 30,
  auto_optimize: false,
  optimizing: false,
  proposal: null,
  ...more,
});

test('says what has passed since optimizing', () => {
  expect(sinceOptimized(status({}))).toBe('512 reviews since you last optimized');
  expect(sinceOptimized(status({ reviews_since: 40, days_since: 35 }))).toBe('35 days and 40 reviews since you last optimized');
  expect(sinceOptimized(status({ fitted_at: null, days_since: null, reviews_since: 600 }))).toBe('600 reviews and never optimized');
});

test('a dismissed suggestion comes back after as many reviews or days again', () => {
  const dismissed = { reviewCount: 2000, day: '2026-11-01' };
  expect(stillDismissed(status({}), null, '2026-11-02')).toBe(false);
  expect(stillDismissed(status({ review_count: 2400 }), dismissed, '2026-11-20')).toBe(true);
  expect(stillDismissed(status({ review_count: 2500 }), dismissed, '2026-11-20')).toBe(false);
  expect(stillDismissed(status({ review_count: 2001 }), dismissed, '2026-12-01')).toBe(false);
});
