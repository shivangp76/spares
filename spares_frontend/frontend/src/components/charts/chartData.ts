// Shaping statistics for the charts: filling in days without data, merging days to fit, and
// formatting values for axes, tooltips and tables.

/** One series of a stacked chart, bottom first. */
export interface Series {
  key: string;
  label: string;
  /** A CSS colour, e.g. `var(--chart-young)`. */
  color: string;
  values: number[];
}

/** `YYYY-MM-DD` for a UTC date, the format the server keys dates by. */
function isoDate(date: Date): string {
  return date.toISOString().slice(0, 10);
}

/** Every date from `start` to `end`, both included, as `YYYY-MM-DD`. Dates are calendar dates, so this counts in UTC to be safe across daylight saving changes. */
export function daysBetween(start: string, end: string): string[] {
  const days: string[] = [];
  const day = new Date(`${start}T00:00:00Z`);
  const last = new Date(`${end}T00:00:00Z`);
  while (day <= last && days.length < 100_000) {
    days.push(isoDate(day));
    day.setUTCDate(day.getUTCDate() + 1);
  }
  return days;
}

/** `values(entry)` for each of `days`, or zeros for a day without an entry. */
export function fillDays<T extends { date: string }>(days: string[], entries: T[], values: (entry: T) => number[], width: number): number[][] {
  const byDate = new Map(entries.map(entry => [entry.date, entry]));
  return days.map(day => {
    const entry = byDate.get(day);
    return entry ? values(entry) : new Array<number>(width).fill(0);
  });
}

/** Columns of per-series values turned into series of per-column values. */
export function toSeries(columns: number[][], specs: Omit<Series, 'values'>[]): Series[] {
  return specs.map((spec, i) => ({ ...spec, values: columns.map(column => column[i] ?? 0) }));
}

/**
 * Merges consecutive columns, adding up their values, until there are at most `max`, so a year of
 * days fits a phone. Each merged column is labeled with its first and last label, e.g.
 * `2026-01-01 – 2026-01-07`.
 */
export function mergeColumns(labels: string[], series: Series[], max: number): { labels: string[]; series: Series[] } {
  if (labels.length <= max || max < 1) return { labels, series };
  const size = Math.ceil(labels.length / max);
  const merged: string[] = [];
  for (let i = 0; i < labels.length; i += size) {
    const last = Math.min(i + size, labels.length) - 1;
    merged.push(last === i ? labels[i] : `${labels[i]} – ${labels[last]}`);
  }
  return {
    labels: merged,
    series: series.map(s => ({
      ...s,
      values: merged.map((_, j) => s.values.slice(j * size, (j + 1) * size).reduce((sum, v) => sum + v, 0)),
    })),
  };
}

/** Each column's stack: the sum of every series. */
export function totals(series: Series[], columns: number): number[] {
  return Array.from({ length: columns }, (_, i) => series.reduce((sum, s) => sum + (s.values[i] ?? 0), 0));
}

/** Running totals, for a forecast's cumulative line. */
export function cumulative(values: number[]): number[] {
  let sum = 0;
  return values.map(value => (sum += value));
}

/** The smallest of 1, 2, 2.5 or 5 times a power of ten that is at least `value`: the top of an axis. */
export function niceMax(value: number): number {
  if (!(value > 0) || !Number.isFinite(value)) return 1;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  for (const step of [1, 2, 2.5, 5, 10]) {
    if (step * magnitude >= value) return step * magnitude;
  }
  return 10 * magnitude;
}

/** `80%`, or `–` when there is nothing to divide. */
export function percent(part: number, whole: number): string {
  return whole > 0 ? `${Math.round((part / whole) * 100)}%` : '–';
}

/** A value for an axis or table: whole above 10, else to one decimal, with digit grouping. */
export function formatValue(value: number): string {
  const digits = Math.abs(value) >= 10 || Number.isInteger(value) ? 0 : 1;
  return value.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits });
}

/** A histogram bucket's label, e.g. `10–20`. */
export function bucketLabel(edges: number[], index: number): string {
  return `${formatValue(edges[index])}–${formatValue(edges[index + 1])}`;
}

/** The hours of the day as axis labels, `00` to `23`. */
export const HOUR_LABELS = Array.from({ length: 24 }, (_, hour) => String(hour).padStart(2, '0'));
