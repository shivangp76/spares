// The statistics page's last responses, kept to show when the server cannot be reached. Statistics
// are never computed on the device: that would mean copying the whole review log into the browser
// and repeating the server's definitions here.
import type { ReviewHistoryResponse, StatisticsGraphsResponse, StatisticsResponse } from '../types/spares';
import { deleteStatisticsEntries, getStatisticsEntry, listStatisticsEntries, putStatisticsEntry } from './db';

/** What the page was asked to show, apart from the date: only today's statistics are kept. */
export interface StatisticsView {
  query: string;
  /** `null` for all time. */
  periodDays: number | null;
  forecastDays: number;
}

export interface CachedStatistics {
  key: string;
  serverUrl: string;
  view: StatisticsView;
  statistics: StatisticsResponse;
  graphs: StatisticsGraphsResponse;
  /** The calendar's, when it was fetched too. */
  history: ReviewHistoryResponse | null;
  fetchedAt: number;
  /** When it was last saved or shown, which decides what is evicted. */
  usedAt: number;
}

/** Where entries are kept: IndexedDB in the app, a `Map` in tests. */
export interface StatisticsCache {
  get(key: string): Promise<CachedStatistics | undefined>;
  list(): Promise<CachedStatistics[]>;
  put(entry: CachedStatistics): Promise<void>;
  remove(keys: string[]): Promise<void>;
}

/** A graphs response for a year is a few tens of kB, so a few views are plenty. */
export const MAX_CACHED_STATISTICS = 10;

export const indexedDbStatisticsCache: StatisticsCache = {
  get: getStatisticsEntry,
  list: listStatisticsEntries,
  put: putStatisticsEntry,
  remove: deleteStatisticsEntries,
};

export function statisticsKey(serverUrl: string, view: StatisticsView): string {
  return JSON.stringify([serverUrl, view.query.trim(), view.periodDays, view.forecastDays]);
}

/** The keys to drop so at most `max` entries remain, the least recently used first. */
export function evictions(entries: CachedStatistics[], max = MAX_CACHED_STATISTICS): string[] {
  return [...entries]
    .sort((a, b) => b.usedAt - a.usedAt)
    .slice(max)
    .map(entry => entry.key);
}

/** Saves a view's statistics after a fetch, evicting the least recently used beyond the limit. */
export async function rememberStatistics(cache: StatisticsCache, entry: CachedStatistics): Promise<void> {
  await cache.put(entry);
  await cache.remove(evictions(await cache.list()));
}

/** A view's saved statistics, marked as just used so they are kept longest. */
export async function recallStatistics(cache: StatisticsCache, key: string, now = Date.now()): Promise<CachedStatistics | undefined> {
  const entry = await cache.get(key);
  if (!entry) return undefined;
  const used = { ...entry, usedAt: now };
  await cache.put(used);
  return used;
}

/** The saved views of `serverUrl`, the most recently used first. */
export async function cachedViews(cache: StatisticsCache, serverUrl: string): Promise<CachedStatistics[]> {
  return (await cache.list())
    .filter(entry => entry.serverUrl === serverUrl)
    .sort((a, b) => b.usedAt - a.usedAt);
}

export function periodLabel(periodDays: number | null): string {
  switch (periodDays) {
    case null: return 'All time';
    case 30: return '1 month';
    case 90: return '3 months';
    case 365: return '1 year';
    default: return `${periodDays} days`;
  }
}

/** E.g. `tag=a · 3 months · 30 day forecast`. */
export function describeView(view: StatisticsView): string {
  return [view.query.trim() || 'All cards', periodLabel(view.periodDays), `${view.forecastDays} day forecast`].join(' · ');
}

/** The offline banner, e.g. `Offline: statistics from 14:05. 3 reviews not counted yet.` */
export function offlineBanner(fetchedAt: number, pending: number, now = new Date()): string {
  const fetched = new Date(fetchedAt);
  const sameDay = fetched.toDateString() === now.toDateString();
  const time = fetched.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  const when = sameDay ? time : `${fetched.toLocaleDateString(undefined, { dateStyle: 'medium' })} ${time}`;
  const notCounted = pending > 0 ? ` ${pending} ${pending === 1 ? 'review' : 'reviews'} not counted yet.` : '';
  return `Offline: statistics from ${when}.${notCounted}`;
}
