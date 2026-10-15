// Reviewing without a connection to the server.
//
// While online, the review page downloads the cards a review would show next, along with what
// they display (`downloadQueue`). Offline, it shows those instead of asking the server, and
// ratings and buries are kept here until they can be sent.
//
// Ratings and buries are sent through the same outbox when online, too. Each carries an id the
// server records, so one whose response was lost is resent without being applied twice.
import { useSyncExternalStore } from 'react';
import {
  fetchFile,
  fetchRenderAsset,
  fetchReviewQueue,
  getCredentials,
  isNetworkError,
  onServerReachability,
  pruneOfflineFiles,
  syncOfflineActions,
} from '../api/client';
import type { GetReviewCardResponse, OfflineStudyActionKind, ReviewFilter } from '../types/spares';
import { lastReviewWidth, widthInputs } from '../typst/width';
import { backPath, backWebPath } from '../utils';
import {
  addPendingAction,
  deletePendingActions,
  getQueue,
  listPendingActions,
  listQueues,
  putQueue,
  removeFromQueues,
  type OfflineQueue,
  type PendingAction,
} from './db';

/** The most cards downloaded per filter */
const QUEUE_SIZE = 200;
/** Tomorrow's cards are downloaded too, for a session offline past midnight */
const DAYS_AHEAD = 1;
/** A queue is downloaded again when the review page is opened this long after the last time */
const STALE_AFTER_MS = 10 * 60 * 1000;
const DEFAULT_SECONDS_PER_CARD = 30;

// Status, for the navbar

export interface OfflineStatus {
  online: boolean;
  /** Actions not yet sent to the server */
  pending: number;
  /** Cards whose content is being downloaded, if a queue is being downloaded */
  downloading: { done: number; total: number } | null;
}

let status: OfflineStatus = { online: navigator.onLine, pending: 0, downloading: null };
const listeners = new Set<() => void>();

function setStatus(update: Partial<OfflineStatus>): void {
  status = { ...status, ...update };
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useOfflineStatus(): OfflineStatus {
  return useSyncExternalStore(subscribe, () => status);
}

function currentServerUrl(): string | null {
  try {
    return getCredentials().serverUrl;
  } catch {
    return null;
  }
}

async function refreshPendingCount(): Promise<void> {
  const serverUrl = currentServerUrl();
  const actions = await listPendingActions();
  setStatus({ pending: actions.filter(action => action.serverUrl === serverUrl).length });
}

// Sending actions

export interface SyncResult {
  /** The event to undo each applied action with, by its id */
  applied: Map<string, number | null>;
  /** Why each rejected action could not be applied, by its id */
  rejected: Map<string, string>;
}

let syncing: Promise<SyncResult> | null = null;

/**
 * Sends the actions waiting to be sent. Rejects with a network error if the server cannot be
 * reached, in which case they stay waiting. One sync runs at a time.
 */
export function syncPendingActions(): Promise<SyncResult> {
  syncing ??= (async () => {
    try {
      return await sendPendingActions();
    } finally {
      syncing = null;
    }
  })();
  return syncing;
}

async function sendPendingActions(): Promise<SyncResult> {
  const result: SyncResult = { applied: new Map(), rejected: new Map() };
  const serverUrl = currentServerUrl();
  // Actions recorded while logged in to another server wait until it is logged in to again
  const actions = (await listPendingActions()).filter(action => action.serverUrl === serverUrl);
  const bySchedulerName = new Map<string, PendingAction[]>();
  for (const action of actions) bySchedulerName.set(action.schedulerName, [...bySchedulerName.get(action.schedulerName) ?? [], action]);
  try {
    for (const [schedulerName, batch] of bySchedulerName) {
      const res = await syncOfflineActions({
        scheduler_name: schedulerName,
        actions: batch.map(({ client_id, performed_at, action }) => ({ client_id, performed_at, action })),
      });
      for (const { client_id, outcome } of res.results) {
        if (outcome === 'Duplicate') result.applied.set(client_id, null);
        else if ('Applied' in outcome) result.applied.set(client_id, outcome.Applied.event_id);
        else result.rejected.set(client_id, outcome.Rejected.message);
      }
      await deletePendingActions(res.results.map(r => r.client_id));
    }
  } finally {
    await refreshPendingCount();
  }
  return result;
}

/** Sends waiting actions now and whenever the connection or the app comes back. */
export function startBackgroundSync(): void {
  // Whether the server can be reached, which `navigator.onLine` cannot tell
  onServerReachability(online => { if (online !== status.online) setStatus({ online }); });
  const sync = () => {
    if (currentServerUrl() === null) return;
    syncPendingActions().catch(e => { if (!isNetworkError(e)) console.error(e); });
  };
  window.addEventListener('online', sync);
  window.addEventListener('offline', () => setStatus({ online: false }));
  document.addEventListener('visibilitychange', () => { if (document.visibilityState === 'visible') sync(); });
  sync();
}

// Performing actions

function newClientId(): string {
  // `randomUUID` is only available on secure origins
  return crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export interface ActionResult {
  clientId: string;
  /** Whether the server has it. If not, it is sent once the server can be reached. */
  sent: boolean;
  /** The event to undo it with on the server, if it was sent and can be undone. */
  eventId: number | null;
}

/**
 * Performs a rating or bury: keeps it to send, then tries to send it. Throws if the server
 * rejected it, and otherwise succeeds whether or not the server could be reached.
 */
export async function performStudyAction(schedulerName: string, action: OfflineStudyActionKind): Promise<ActionResult> {
  const { serverUrl } = getCredentials();
  const pending: PendingAction = {
    client_id: newClientId(),
    performed_at: new Date().toISOString(),
    action,
    serverUrl,
    schedulerName,
    addedAt: Date.now(),
  };
  await addPendingAction(pending);
  const cardId = 'Rate' in action ? action.Rate.card_id : action.Bury.card_id;
  // Reviewed, so not to be shown again from a downloaded queue
  await removeFromQueues(cardId);
  await refreshPendingCount();

  const notSent = { clientId: pending.client_id, sent: false, eventId: null };
  try {
    let result = await syncPendingActions();
    // A sync that was already running when this was added did not include it
    const included = (r: SyncResult) => r.applied.has(pending.client_id) || r.rejected.has(pending.client_id);
    if (!included(result)) result = await syncPendingActions();
    if (!included(result)) return notSent;
    const rejected = result.rejected.get(pending.client_id);
    if (rejected !== undefined) throw new Error(`The server rejected it: ${rejected}`);
    return { clientId: pending.client_id, sent: true, eventId: result.applied.get(pending.client_id) ?? null };
  } catch (e) {
    if (isNetworkError(e)) return notSent;
    throw e;
  }
}

/** Takes back an action that has not been sent yet. Returns whether it was still waiting. */
export async function cancelPendingAction(clientId: string): Promise<boolean> {
  const waiting = (await listPendingActions()).some(action => action.client_id === clientId);
  if (waiting) await deletePendingActions([clientId]);
  await refreshPendingCount();
  return waiting;
}

/** Drops a card from the downloaded queues, e.g. after it was suspended online. */
export function forgetQueuedCard(cardId: number): Promise<void> {
  return removeFromQueues(cardId);
}

// Downloading queues

/** The rendered files a card shows, relative to the files directory. Typst cards are compiled instead. */
function cardFiles(card: GetReviewCardResponse): string[] {
  if (card.browser_sources) return [];
  if (card.card_front_web_path !== undefined) {
    const back = backWebPath(card);
    return back === undefined ? [card.card_front_web_path] : [card.card_front_web_path, back];
  }
  return [card.card_front_rendered_path, backPath(card)];
}

/** Cards downloaded at once. Typst cards are compiled one at a time regardless. */
const DOWNLOAD_CONCURRENCY = 4;

async function downloadCardContent(card: GetReviewCardResponse): Promise<void> {
  try {
    // Kept by `fetchFile` for offline use
    await Promise.all(cardFiles(card).map(fetchFile));
    // Kept by `fetchRenderAsset`
    if (card.audio) await Promise.all([...card.audio.front, ...card.audio.back].map(fetchRenderAsset));
    if (card.browser_sources) {
      // Compiling fetches, and keeps, the compiler and the files the sources read
      const { compileTypstInBackground } = await import('../typst/compiler');
      await compileTypstInBackground(card.browser_sources.card_front);
      await compileTypstInBackground(card.browser_sources.card_back);
      // Leave the page responsive between compiles
      await new Promise(resolve => setTimeout(resolve, 0));
    }
  } catch (e) {
    if (isNetworkError(e)) throw e;
    // The card shows the same error offline as it would online
    console.error(`Downloading card ${card.card_id}`, e);
  }
}

async function downloadContent(cards: GetReviewCardResponse[]): Promise<void> {
  let next = 0;
  let done = 0;
  setStatus({ downloading: { done, total: cards.length } });
  const worker = async () => {
    while (next < cards.length) {
      await downloadCardContent(cards[next++]);
      done += 1;
      setStatus({ downloading: { done, total: cards.length } });
    }
  };
  try {
    await Promise.all(Array.from({ length: DOWNLOAD_CONCURRENCY }, worker));
  } finally {
    setStatus({ downloading: null });
  }
}

const downloads = new Map<string, Promise<OfflineQueue>>();

/**
 * Downloads the cards a review with `filter` would show next, and what they display, replacing
 * any queue downloaded for `key` before. One download per key runs at a time.
 */
export function downloadQueue(key: string, filter: ReviewFilter | null, sessionInfo: string | null): Promise<OfflineQueue> {
  let download = downloads.get(key);
  if (!download) {
    download = (async () => {
      try {
        // So the server does not return cards that were already reviewed offline
        await syncPendingActions();
        const { serverUrl } = getCredentials();
        const res = await fetchReviewQueue({ filter, limit: QUEUE_SIZE, days_ahead: DAYS_AHEAD });
        const todayCount = Object.values(res.cards[0]?.cards_left_by_state ?? {}).reduce((a, b) => a + b, 0);
        const queue: OfflineQueue = {
          key,
          serverUrl,
          filter,
          sessionInfo,
          fetchedAt: Date.now(),
          cards: res.cards,
          secondsPerCard: todayCount > 0 ? res.cards[0].time_estimate / todayCount : DEFAULT_SECONDS_PER_CARD,
        };
        await putQueue(queue);
        // Ask the browser not to evict what is downloaded when storage runs low
        navigator.storage?.persist?.().catch(console.error);
        await downloadContent(queue.cards);
        const kept = (await listQueues()).flatMap(q => q.cards.flatMap(cardFiles));
        await pruneOfflineFiles(kept);
        return queue;
      } finally {
        downloads.delete(key);
      }
    })();
    downloads.set(key, download);
  }
  return download;
}

/** Downloads the queue for `key` unless it was downloaded recently. Errors are logged. */
export function refreshQueue(key: string, filter: ReviewFilter | null, sessionInfo: string | null): void {
  (async () => {
    const queue = await getQueue(key);
    const serverUrl = currentServerUrl();
    if (queue && queue.serverUrl === serverUrl && Date.now() - queue.fetchedAt < STALE_AFTER_MS) return;
    await downloadQueue(key, filter, sessionInfo);
  })().catch(e => { if (!isNetworkError(e)) console.error(e); });
}

// Reviewing offline

/** The downloaded queue for `key`, if there is one for the server logged in to. */
export async function getOfflineQueue(key: string): Promise<OfflineQueue | null> {
  const queue = await getQueue(key);
  return queue && queue.serverUrl === currentServerUrl() ? queue : null;
}

function endOfToday(now: Date): Date {
  const end = new Date(now);
  end.setHours(24, 0, 0, 0);
  return end;
}

/**
 * The next card to review from the queue for `key`, with the counts of what is left, or `null` if
 * nothing in it is due today.
 */
export async function nextOfflineCard(key: string, now: Date = new Date()): Promise<GetReviewCardResponse | null> {
  const queue = await getOfflineQueue(key);
  if (!queue) return null;
  const limit = endOfToday(now).getTime();
  const due = queue.cards.filter(card => new Date(card.due).getTime() < limit);
  if (due.length === 0) return null;
  const cardsLeftByState: Record<string, number> = {};
  for (const card of due) cardsLeftByState[card.card_state] = (cardsLeftByState[card.card_state] ?? 0) + 1;
  return {
    ...due[0],
    cards_left_by_state: cardsLeftByState,
    time_estimate: Math.round(due.length * queue.secondsPerCard),
  };
}

/**
 * Compiles both sides of the next card in the queue for `key`, at the width review cards were last
 * shown at, so that the card shows at once if the server gives it next.
 */
export async function precompileNextCard(key: string): Promise<void> {
  const width = lastReviewWidth();
  const card = width === null ? null : await nextOfflineCard(key);
  if (width === null || !card?.browser_sources) return;
  const { precompileTypst } = await import('../typst/compiler');
  const inputs = widthInputs(width);
  await Promise.all([precompileTypst(card.browser_sources.card_front, inputs), precompileTypst(card.browser_sources.card_back, inputs)]);
}

/** Puts a card back at the front of the queue for `key`, e.g. after its rating was undone. */
export async function returnToQueue(key: string, card: GetReviewCardResponse): Promise<void> {
  const queue = await getQueue(key);
  if (!queue || queue.cards.some(c => c.card_id === card.card_id)) return;
  await putQueue({ ...queue, cards: [{ ...card, due: new Date(0).toISOString() }, ...queue.cards] });
}
