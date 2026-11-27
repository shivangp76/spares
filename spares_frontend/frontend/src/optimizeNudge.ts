/** When to suggest optimizing the scheduler's parameters, for `components/OptimizeNudge.tsx`. */
import type { OptimizeStatusResponse } from './types/spares';

export const DISMISSED_KEY = 'spares_optimize_nudge_dismissed';

/** When the suggestion to optimize was dismissed, and how many reviews there were then. */
export interface Dismissed {
  reviewCount: number;
  /** `YYYY-MM-DD` */
  day: string;
}

export function today(): string {
  return new Date().toISOString().slice(0, 10);
}

export function readDismissed(): Dismissed | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(DISMISSED_KEY) ?? 'null');
    if (typeof value !== 'object' || value === null) return null;
    const { reviewCount, day } = value as Partial<Dismissed>;
    return typeof reviewCount === 'number' && typeof day === 'string' ? { reviewCount, day } : null;
  } catch {
    return null;
  }
}

/** Whether a dismissed suggestion stays hidden: until as many reviews or days have passed again as
    the host waits for after optimizing. */
export function stillDismissed(status: OptimizeStatusResponse, dismissed: Dismissed | null, now: string): boolean {
  if (!dismissed) return false;
  const days = (Date.parse(now) - Date.parse(dismissed.day)) / 86_400_000;
  return status.review_count - dismissed.reviewCount < status.after_reviews && days < status.after_days;
}

/** What has passed since the parameters were fitted, e.g. "512 reviews since you last optimized". */
export function sinceOptimized(status: OptimizeStatusResponse): string {
  const reviews = `${status.reviews_since.toLocaleString()} review${status.reviews_since === 1 ? '' : 's'}`;
  if (status.fitted_at === null) return `${reviews} and never optimized`;
  if (status.reviews_since >= status.after_reviews || status.days_since === null)
    return `${reviews} since you last optimized`;
  return `${status.days_since} days and ${reviews} since you last optimized`;
}
