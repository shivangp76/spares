import { expect, test } from 'bun:test';
import type { StatisticsGraphsResponse, StatisticsResponse } from '../types/spares';
import { upgradeDatabase } from './db';
import {
  cachedViews,
  describeView,
  evictions,
  MAX_CACHED_STATISTICS,
  offlineBanner,
  recallStatistics,
  rememberStatistics,
  statisticsKey,
  type CachedStatistics,
  type StatisticsCache,
  type StatisticsView,
} from './statistics';

function memoryCache(): StatisticsCache & { entries: Map<string, CachedStatistics> } {
  const entries = new Map<string, CachedStatistics>();
  return {
    entries,
    get: async key => entries.get(key),
    list: async () => [...entries.values()],
    put: async entry => { entries.set(entry.key, entry); },
    remove: async keys => { for (const key of keys) entries.delete(key); },
  };
}

const SERVER = 'https://spares.example';

function entry(view: StatisticsView, usedAt: number, serverUrl = SERVER): CachedStatistics {
  return {
    key: statisticsKey(serverUrl, view),
    serverUrl,
    view,
    statistics: { cards_studied_count: usedAt } as StatisticsResponse,
    graphs: {} as StatisticsGraphsResponse,
    history: null,
    fetchedAt: usedAt,
    usedAt,
  };
}

const view = (query: string, periodDays: number | null = 30): StatisticsView => ({ query, periodDays, forecastDays: 30 });

test('views are told apart by server, query, period and forecast', () => {
  expect(statisticsKey(SERVER, view('tag=a'))).toBe(statisticsKey(SERVER, view(' tag=a ')));
  expect(statisticsKey(SERVER, view('tag=a'))).not.toBe(statisticsKey(SERVER, view('tag=a', null)));
  expect(statisticsKey(SERVER, view(''))).not.toBe(statisticsKey('https://other.example', view('')));
});

test('the least recently used entry is evicted past the limit', async () => {
  const cache = memoryCache();
  for (let i = 0; i < MAX_CACHED_STATISTICS; i++) await rememberStatistics(cache, entry(view(`q${i}`), i));
  expect(cache.entries.size).toBe(MAX_CACHED_STATISTICS);
  // q0 was used last, so q1 is now the oldest
  await recallStatistics(cache, statisticsKey(SERVER, view('q0')), 100);
  await rememberStatistics(cache, entry(view('new'), 101));
  expect(cache.entries.size).toBe(MAX_CACHED_STATISTICS);
  expect(cache.entries.has(statisticsKey(SERVER, view('q1')))).toBe(false);
  expect(cache.entries.has(statisticsKey(SERVER, view('q0')))).toBe(true);
});

test('evictions keeps the most recently used', () => {
  const entries = [entry(view('a'), 1), entry(view('b'), 3), entry(view('c'), 2)];
  expect(evictions(entries, 2)).toEqual([statisticsKey(SERVER, view('a'))]);
  expect(evictions(entries, 5)).toEqual([]);
});

test('a cached entry is there to show when the fetch fails', async () => {
  const cache = memoryCache();
  await rememberStatistics(cache, entry(view('tag=a'), 5));
  const shown = await recallStatistics(cache, statisticsKey(SERVER, view('tag=a')), 9);
  expect(shown?.statistics.cards_studied_count).toBe(5);
  expect(shown?.usedAt).toBe(9);
  expect(await recallStatistics(cache, statisticsKey(SERVER, view('tag=b')))).toBeUndefined();
});

test('the views to pick from are this server’s, the latest first', async () => {
  const cache = memoryCache();
  await rememberStatistics(cache, entry(view('a'), 1));
  await rememberStatistics(cache, entry(view('b'), 2));
  await rememberStatistics(cache, entry(view('c'), 3, 'https://other.example'));
  expect((await cachedViews(cache, SERVER)).map(e => e.view.query)).toEqual(['b', 'a']);
});

test('views and the banner read naturally', () => {
  expect(describeView(view('', 90))).toBe('All cards · 3 months · 30 day forecast');
  const now = new Date(2026, 9, 16, 15, 0);
  const banner = offlineBanner(new Date(2026, 9, 16, 14, 5).getTime(), 3, now);
  expect(banner).toStartWith('Offline: statistics from ');
  expect(banner).toEndWith('. 3 reviews not counted yet.');
  expect(offlineBanner(now.getTime(), 0, now)).not.toContain('not counted');
  expect(offlineBanner(now.getTime(), 1, now)).toEndWith('1 review not counted yet.');
});

test('upgrading the database from version 1 keeps the queues and outbox', () => {
  const created: string[] = [];
  const db = { createObjectStore: (name: string) => { created.push(name); return {} as IDBObjectStore; } };
  upgradeDatabase(db, 1);
  // Only the new store: the existing ones are neither recreated nor cleared
  expect(created).toEqual(['statistics']);
  created.length = 0;
  upgradeDatabase(db, 0);
  expect(created).toEqual(['queues', 'outbox', 'statistics']);
  created.length = 0;
  upgradeDatabase(db, 2);
  expect(created).toEqual([]);
});
