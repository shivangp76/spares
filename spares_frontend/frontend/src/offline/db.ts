// What the app keeps in IndexedDB to review offline: the review queues downloaded while online,
// and the actions performed offline that are yet to be sent to the server.
import type { OfflineStudyAction, ReviewFilter, ReviewQueueCard } from '../types/spares';

/** The cards a review would show next, downloaded to review offline. */
export interface OfflineQueue {
  /** The filter as the user entered it, e.g. `query:tag=a` (see `queueKey`). */
  key: string;
  serverUrl: string;
  /** What the filter resolved to when the queue was downloaded, so it can be reviewed without asking the server. */
  filter: ReviewFilter | null;
  /** Shown while reviewing, like an online session's. */
  sessionInfo: string | null;
  fetchedAt: number;
  /** Cards not yet reviewed, in the order to show them. */
  cards: ReviewQueueCard[];
  /** `time_estimate` per card, from the server's estimate when the queue was downloaded. */
  secondsPerCard: number;
}

/** An action performed offline, waiting to be sent. */
export interface PendingAction extends OfflineStudyAction {
  serverUrl: string;
  schedulerName: string;
  /** When it was added, which orders the actions to undo them. */
  addedAt: number;
}

const DB_NAME = 'spares-offline';
const QUEUES = 'queues';
const OUTBOX = 'outbox';

let opened: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  opened ??= new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore(QUEUES, { keyPath: 'key' });
      request.result.createObjectStore(OUTBOX, { keyPath: 'client_id' });
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  // Allow retrying, e.g. after the user allowed storage
  opened.catch(() => { opened = null; });
  return opened;
}

function settle<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function complete(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error);
  });
}

/** Runs `action` on `store` in a transaction of its own, resolving once it is committed. */
async function run<T>(store: string, mode: IDBTransactionMode, action: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const transaction = (await open()).transaction(store, mode);
  const [result] = await Promise.all([settle(action(transaction.objectStore(store))), complete(transaction)]);
  return result;
}

/** Identifies a filter as the user entered it, e.g. on the review page's landing form. */
export function queueKey(mode: string, input: string): string {
  return `${mode}:${input.trim()}`;
}

export async function getQueue(key: string): Promise<OfflineQueue | undefined> {
  return run<OfflineQueue | undefined>(QUEUES, 'readonly', store => store.get(key));
}

export async function listQueues(): Promise<OfflineQueue[]> {
  return run<OfflineQueue[]>(QUEUES, 'readonly', store => store.getAll());
}

export async function putQueue(queue: OfflineQueue): Promise<void> {
  await run(QUEUES, 'readwrite', store => store.put(queue));
}

/** Drops a card from every queue, e.g. once it was reviewed online so it is not shown again offline. */
export async function removeFromQueues(cardId: number): Promise<void> {
  for (const queue of await listQueues()) {
    const cards = queue.cards.filter(card => card.card_id !== cardId);
    if (cards.length !== queue.cards.length) await putQueue({ ...queue, cards });
  }
}

export async function clearOfflineQueues(): Promise<void> {
  await run(QUEUES, 'readwrite', store => store.clear());
}

export async function listPendingActions(): Promise<PendingAction[]> {
  const actions = await run<PendingAction[]>(OUTBOX, 'readonly', store => store.getAll());
  return actions.sort((a, b) => a.addedAt - b.addedAt);
}

export async function addPendingAction(action: PendingAction): Promise<void> {
  await run(OUTBOX, 'readwrite', store => store.add(action));
}

export async function deletePendingActions(clientIds: string[]): Promise<void> {
  if (clientIds.length === 0) return;
  const transaction = (await open()).transaction(OUTBOX, 'readwrite');
  for (const id of clientIds) transaction.objectStore(OUTBOX).delete(id);
  await complete(transaction);
}
