import { useCallback, useState } from 'react';

const STORAGE_KEY = 'spares_review_bury_until';

/** Today's local date as `YYYY-MM-DD`, the format of an `<input type="date">`. */
export function localDateString(date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function load(): Record<string, string> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed as Record<string, string> : {};
  } catch {
    return {};
  }
}

function store(dates: Record<string, string>) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(dates));
  } catch {
    // The date is a convenience, so a storage failure only means it is picked again next session
  }
}

/** The date remembered for `key`, unless it has passed, so an old exam's date is not reused. */
function current(key: string): string | null {
  const date = load()[key];
  return date && date >= localDateString() ? date : null;
}

/**
 * The date to bury cards until, e.g. an exam's, remembered in `localStorage` for each review
 * session `key` (see `queueKey`), so studying the same tag again offers it without picking it
 * again. A date that has passed reads as none.
 */
export function useBuryUntilDate(key: string) {
  const [state, setState] = useState(() => ({ key, date: current(key) }));
  // Re-read when the session changes, during render rather than in an effect, so the new
  // session's first card never shows the last session's date
  let date = state.date;
  if (state.key !== key) {
    date = current(key);
    setState({ key, date });
  }

  const save = useCallback((value: string | null) => {
    const today = localDateString();
    // Dropping dates that have passed keeps the stored map from growing with every exam
    const dates = Object.fromEntries(Object.entries(load()).filter(([k, d]) => k !== key && d >= today));
    if (value) dates[key] = value;
    store(dates);
    setState({ key, date: value });
  }, [key]);

  return [date, save] as const;
}

/** When a card buried until `date` comes back: the start of the next local day, after the exam. */
export function dayAfter(date: string): Date {
  const [y, m, d] = date.split('-').map(Number);
  return new Date(y, m - 1, d + 1);
}

/** A bury-until date for a button, e.g. `Nov 4`. */
export function formatBuryUntil(date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}
