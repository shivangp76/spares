import type {
  CardResponse,
  CardReviewEntry,
  CreateNoteRequest,
  CreateTagRequest,
  Credentials,
  EventSummary,
  ForgetCardResponse,
  GetReviewCardResponse,
  ImageOcclusionData,
  ImageOcclusionEditorConfig,
  MatchedKeywordResponse,
  NoteLink,
  NoteRenderResponse,
  NoteResponse,
  NotesSelector,
  ParserResponse,
  Rating,
  RedoEventResponse,
  ReviewConfig,
  ReviewFilter,
  ReviewHistoryResponse,
  ReviewQueueRequest,
  ReviewQueueResponse,
  ReviewSnapshotResponse,
  StatisticsResponse,
  SubmitStudyActionRequest,
  SubmitStudyActionResponse,
  SyncOfflineActionsRequest,
  SyncOfflineActionsResponse,
  TagResponse,
  UndoEventResponse,
  UnmatchedKeywordResponse,
  UpdateCardsRequest,
  UpdateCardsResponse,
  UpdateTagRequest,
} from '../types/spares';
import { clearOfflineQueues } from '../offline/db';

const STORAGE_KEY = 'spares_credentials';

export function getCredentials(): Credentials {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) throw new Error('Not authenticated');
  return JSON.parse(raw) as Credentials;
}

export function saveCredentials(creds: Credentials): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(creds));
  clearCaches();
  clearOfflineCopies().catch(console.error);
}

export function clearCredentials(): void {
  localStorage.removeItem(STORAGE_KEY);
  clearCaches();
  clearOfflineCopies().catch(console.error);
}

// Responses kept in memory until a reload or a credentials change, so revisiting a page doesn't wait
// on the server. Ratings and the review config rarely change, so they are fetched once.
const memoized = new Map<string, Promise<unknown>>();
// List pages are shown from here while they are refetched. Any change may affect them, so they are
// dropped on every request that isn't a GET.
const listPages = new Map<string, unknown[]>();
// Today's statistics per scheduler, shown on the review page while they are refetched
const todayStatistics = new Map<string, StatisticsResponse>();
// Likewise the review history for the review page's heatmap
let reviewHistory: ReviewHistoryResponse | undefined;

function clearCaches(): void {
  memoized.clear();
  listPages.clear();
  todayStatistics.clear();
  reviewHistory = undefined;
}

function memoize<T>(key: string, fetch: () => Promise<T>): Promise<T> {
  let result = memoized.get(key) as Promise<T> | undefined;
  if (!result) {
    const fetched = fetch();
    // Allow retrying after e.g. a network error
    fetched.catch(() => { if (memoized.get(key) === fetched) memoized.delete(key); });
    memoized.set(key, fetched);
    result = fetched;
  }
  return result;
}

const OFFLINE_COPY_PREFIX = 'spares_offline_copy:';

/** `fetch`'s result, keeping a copy to return instead when the server cannot be reached. For small responses that reviewing offline needs. */
async function withOfflineCopy<T>(key: string, fetch: () => Promise<T>): Promise<T> {
  try {
    const result = await fetch();
    try {
      localStorage.setItem(OFFLINE_COPY_PREFIX + key, JSON.stringify(result));
    } catch (e) {
      console.error(e);
    }
    return result;
  } catch (e) {
    const copy = isNetworkError(e) ? localStorage.getItem(OFFLINE_COPY_PREFIX + key) : null;
    if (copy === null) throw e;
    return JSON.parse(copy) as T;
  }
}

async function cacheListPage<T>(key: string, fetch: Promise<T[]>): Promise<T[]> {
  const page = await fetch;
  listPages.set(key, page);
  return page;
}

function authHeaders(): HeadersInit {
  const { apiKey } = getCredentials();
  return {
    Authorization: `Bearer ${apiKey}`,
    'Content-Type': 'application/json',
  };
}

/** The server could not be reached, e.g. because the device is offline. */
export class NetworkError extends Error {
  constructor(label: string) {
    super(`${label} failed: the server could not be reached`);
    this.name = 'NetworkError';
  }
}

export function isNetworkError(e: unknown): boolean {
  return e instanceof NetworkError;
}

let reachabilityListener: ((reachable: boolean) => void) | null = null;

/** Called with whether the server could be reached, after every request to it. */
export function onServerReachability(listener: (reachable: boolean) => void): void {
  reachabilityListener = listener;
}

/** `fetch`, throwing a `NetworkError` if the server cannot be reached. */
async function send(label: string, url: string, init: RequestInit = {}): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(url, init);
  } catch (e) {
    // An abort is the caller's, not the network's
    if (e instanceof DOMException && e.name === 'AbortError') throw e;
    reachabilityListener?.(false);
    throw new NetworkError(label);
  }
  reachabilityListener?.(true);
  return res;
}

// Copies of server responses kept for offline use, by URL without the API key. Rendered files are
// kept while a downloaded review queue uses them (see src/offline), and the files Typst sources
// read until logout.
const FILES_CACHE = 'spares-offline-files';
const RENDER_ASSETS_CACHE = 'spares-offline-render-assets';

/**
 * Fetches `url` from the server, keeping a copy in `cacheName`, and falls back to the copy if the
 * server cannot be reached. Cache Storage is only available on secure origins (HTTPS or
 * localhost), so elsewhere this is a plain fetch.
 */
async function sendKeepingCopy(label: string, url: string, cacheName: string): Promise<Response> {
  const cache = typeof caches === 'undefined' ? null : await caches.open(cacheName).catch(() => null);
  try {
    // Revalidated, so the browser's HTTP cache does not answer for a server that cannot be reached
    const res = await send(label, url, { headers: authHeaders(), cache: 'no-cache' });
    // 404s are kept too, so a file the server does not have is not requested offline
    if (cache && (res.ok || res.status === 404)) await cache.put(url, res.clone()).catch(console.error);
    return res;
  } catch (e) {
    const copy = await cache?.match(url, { ignoreVary: true });
    if (copy && isNetworkError(e)) return copy;
    throw e;
  }
}

/** Drops copies of rendered files that are not in `keep` (paths relative to the files directory). */
export async function pruneOfflineFiles(keep: Iterable<string>): Promise<void> {
  if (typeof caches === 'undefined') return;
  const keepUrls = new Set([...keep].map(fileUrl));
  const cache = await caches.open(FILES_CACHE);
  for (const request of await cache.keys()) {
    if (!keepUrls.has(request.url)) await cache.delete(request);
  }
}

/** Drops everything downloaded for offline use, except actions that are yet to be synced. */
async function clearOfflineCopies(): Promise<void> {
  for (const key of Object.keys(localStorage)) {
    if (key.startsWith(OFFLINE_COPY_PREFIX)) localStorage.removeItem(key);
  }
  if (typeof caches !== 'undefined') {
    await caches.delete(FILES_CACHE);
    await caches.delete(RENDER_ASSETS_CACHE);
  }
  await clearOfflineQueues();
}

/** Throws with the server's `{ message }` (or plain-text body) when present, falling back to the status code. */
async function throwIfNotOk(res: Response, label: string): Promise<void> {
  if (res.ok) return;
  let message = `${label} failed: ${res.status}`;
  const text = await res.text().catch(() => '');
  try {
    const body = JSON.parse(text) as { message?: unknown };
    if (typeof body.message === 'string') message = `${label} failed: ${body.message}`;
  } catch {
    // Not JSON, e.g. a request the server rejected before reaching a handler
    if (text) message = `${label} failed: ${text}`;
  }
  throw new Error(message);
}

async function apiFetch<T>(label: string, path: string, init: RequestInit = {}): Promise<T> {
  const res = await apiSend(label, path, init);
  return res.json() as Promise<T>;
}

/** Like `apiFetch`, for endpoints whose response body is empty. */
async function apiSend(label: string, path: string, init: RequestInit = {}): Promise<Response> {
  const { serverUrl } = getCredentials();
  if ((init.method ?? 'GET') !== 'GET') listPages.clear();
  const res = await send(label, `${serverUrl}${path}`, { ...init, headers: authHeaders() });
  await throwIfNotOk(res, label);
  return res;
}

function pageQuery(page: number, limit: number): string {
  return `page=${page}&limit=${limit}`;
}

export function fileUrl(relativePath: string): string {
  const { serverUrl } = getCredentials();
  return `${serverUrl.replace(/\/$/, '')}/files/${relativePath}`;
}

/** Downloads a rendered file, which needs the API key like every other request. Offline, the copy kept from the last download is returned. */
export async function fetchFile(relativePath: string): Promise<Blob> {
  const res = await sendKeepingCopy('Loading file', fileUrl(relativePath), FILES_CACHE);
  await throwIfNotOk(res, 'Loading file');
  return res.blob();
}

/** Package specs (`namespace/name/version`) that browser-rendered sources import. */
export async function listRenderPackages(): Promise<string[]> {
  const { serverUrl } = getCredentials();
  const url = `${serverUrl}/api/render-assets/packages`;
  const res = await sendKeepingCopy('Render packages fetch', url, RENDER_ASSETS_CACHE);
  if (!res.ok) throw new Error(`Render packages fetch failed: ${res.status}`);
  return res.json();
}

/** Typst files that browser-rendered sources read, e.g. preambles, by the absolute paths the sources use. */
export async function listRenderSources(): Promise<string[]> {
  const { serverUrl } = getCredentials();
  const url = `${serverUrl}/api/render-assets/sources`;
  const res = await sendKeepingCopy('Render sources fetch', url, RENDER_ASSETS_CACHE);
  if (!res.ok) throw new Error(`Render sources fetch failed: ${res.status}`);
  return res.json();
}

/** A file a browser-rendered source reads, by the absolute path the source uses. `null` if the server does not have it. */
export async function fetchRenderAsset(path: string): Promise<Uint8Array | null> {
  const { serverUrl } = getCredentials();
  const url = `${serverUrl}/api/render-assets?path=${encodeURIComponent(path)}`;
  const res = await sendKeepingCopy('Render asset fetch', url, RENDER_ASSETS_CACHE);
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`Render asset fetch failed for ${path}: ${res.status}`);
  return new Uint8Array(await res.arrayBuffer());
}

export async function getImageOcclusionEditorConfig(): Promise<ImageOcclusionEditorConfig> {
  return memoize('image-occlusion-editor-config', async () => {
    const res = await apiSend('Image occlusion editor config', '/api/image-occlusions/editor-config');
    return res.json();
  });
}

/** Stores a new image occlusion. `snippet` is its block in the parser's syntax, ready to insert into a note. */
export async function createImageOcclusion(
  parserId: number,
  image: File,
  clozesSvg: string,
): Promise<{ image_occlusion: ImageOcclusionData; snippet: string }> {
  const { serverUrl, apiKey } = getCredentials();
  const form = new FormData();
  form.append('parser_id', String(parserId));
  form.append('image', image, image.name);
  form.append('clozes', clozesSvg);
  // The browser sets the multipart Content-Type, with its boundary
  const res = await fetch(`${serverUrl}/api/image-occlusions`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${apiKey}` },
    body: form,
  });
  await throwIfNotOk(res, 'Save image occlusion');
  return res.json();
}

/** Stores an image pasted or dropped into a note. `snippet` shows it in the parser's syntax, ready to insert into the note. */
export async function uploadImage(parserId: number, image: File): Promise<{ path: string; snippet: string }> {
  const { serverUrl, apiKey } = getCredentials();
  const form = new FormData();
  form.append('parser_id', String(parserId));
  form.append('image', image, image.name);
  // The browser sets the multipart Content-Type, with its boundary
  const res = await fetch(`${serverUrl}/api/images`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${apiKey}` },
    body: form,
  });
  await throwIfNotOk(res, `Upload ${image.name}`);
  return res.json();
}

/** Stores an audio clip recorded for a note. `snippet` links to it in the parser's syntax, ready to insert into the note. */
export async function uploadAudio(parserId: number, audio: File): Promise<{ path: string; snippet: string }> {
  const { serverUrl, apiKey } = getCredentials();
  const form = new FormData();
  form.append('parser_id', String(parserId));
  form.append('audio', audio, audio.name);
  // The browser sets the multipart Content-Type, with its boundary
  const res = await fetch(`${serverUrl}/api/audio`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${apiKey}` },
    body: form,
  });
  await throwIfNotOk(res, 'Upload recording');
  return res.json();
}

/** Has the server download and store an image dragged from a web page, which browsers hand over
    by URL only. Like `uploadImage`, `snippet` shows it in the parser's syntax. */
export async function uploadImageUrl(parserId: number, url: string): Promise<{ path: string; snippet: string }> {
  return apiFetch('Download image', '/api/images/url', {
    method: 'POST',
    body: JSON.stringify({ parser_id: parserId, url }),
  });
}

/** A file of a stored image occlusion, by the absolute path a note refers to it by. */
export async function fetchImageOcclusionFile(path: string): Promise<Blob> {
  const res = await apiSend('Image occlusion file', `/api/image-occlusions/file?path=${encodeURIComponent(path)}`);
  return res.blob();
}

/** The note's image occlusions, in the order they appear in it. */
export async function listNoteImageOcclusions(noteId: number): Promise<ImageOcclusionData[]> {
  return apiFetch('Image occlusions', `/api/notes/${noteId}/image-occlusions`);
}

/** Replaces the clozes of the note's `index`th image occlusion (0 based). The server stores them as
    new files and updates the note, so its cards follow and the change can be undone. */
export async function updateNoteImageOcclusion(noteId: number, index: number, clozesSvg: string): Promise<NoteResponse> {
  const body = await apiFetch<{ notes: NoteResponse[]; event_id: number | null }>(
    'Save image occlusion',
    `/api/notes/${noteId}/image-occlusions/${index}`,
    { method: 'PUT', body: JSON.stringify({ clozes: clozesSvg }) },
  );
  return body.notes[0];
}

export async function postReview(filter?: ReviewFilter): Promise<GetReviewCardResponse | null> {
  return apiFetch('Review fetch', '/api/review', {
    method: 'POST',
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/**
 * When a review with `filter` that has nothing to show now can show its next card: a learning card
 * due later today, once it comes within the learn ahead limit. `null` when the review is done for
 * the day.
 */
export async function nextReviewAvailableAt(filter?: ReviewFilter): Promise<string | null> {
  return apiFetch('Next review time fetch', '/api/review/next-available', {
    method: 'POST',
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/** Fetches a specific card in review form, e.g. to refresh it after its note was edited. `null` if it no longer exists. */
export async function getReviewCardById(cardId: number, filter?: ReviewFilter): Promise<GetReviewCardResponse | null> {
  return apiFetch('Review card fetch', `/api/review/card/${cardId}`, {
    method: 'POST',
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/** Resolves a query that uses `limit` into a filtered tag, built once per day. */
export async function createReviewSnapshot(query: string): Promise<ReviewSnapshotResponse> {
  return apiFetch('Review snapshot', '/api/review/snapshot', {
    method: 'POST',
    body: JSON.stringify({ query }),
  });
}

export async function getReviewConfig(): Promise<ReviewConfig> {
  return memoize('reviewConfig', () => withOfflineCopy('reviewConfig', () => apiFetch('Review config fetch', '/api/review/config')));
}

export async function getTagByName(name: string): Promise<TagResponse> {
  return apiFetch('Tag fetch', `/api/tags/name/${encodeURIComponent(name)}`);
}

export async function getNote(id: number): Promise<NoteResponse> {
  return apiFetch('Note fetch', `/api/notes/${id}`);
}

/** Where to find the note's rendered file, or its source if it is compiled in the browser. */
export async function getNoteRender(id: number): Promise<NoteRenderResponse> {
  return apiFetch('Note render fetch', `/api/notes/${id}/render`);
}

export async function getCardsForNote(noteId: number): Promise<CardResponse[]> {
  return apiFetch('Cards fetch', `/api/cards/note_id/${noteId}`);
}

export async function updateCards(req: UpdateCardsRequest): Promise<UpdateCardsResponse> {
  return apiFetch('Card update', '/api/cards', { method: 'PATCH', body: JSON.stringify(req) });
}

export async function forgetCard(cardId: number): Promise<ForgetCardResponse> {
  return apiFetch('Forget card', `/api/cards/${cardId}/forget`, { method: 'POST' });
}

/** Adds a tag to a note, leaving its other tags unchanged. Returns the event id. */
export async function tagNote(noteId: number, tag: string): Promise<number | null> {
  const body = await apiFetch<{ event_id: number | null }>('Tag note', '/api/notes', {
    method: 'PATCH',
    body: JSON.stringify({
      selector: { Ids: [noteId] },
      tags: { ModifyTags: { tags_to_remove: null, tags_to_add: [tag] } },
    }),
  });
  return body.event_id;
}

/** Regenerates a note's rendered files (and its cards' and linked notes'), as the CLI does after syncing a note. */
export async function renderNote(noteId: number): Promise<void> {
  await apiSend('Render note', '/api/notes/generate_files', {
    method: 'POST',
    body: JSON.stringify({
      selector: { Ids: [noteId] },
      immutable_note_ids: null,
      overridden_output_raw_dir: null,
      include_linked_notes: true,
      include_cards: true,
      generate_rendered: true,
      force_generate_rendered: false,
    }),
  });
}

/** Undoes `eventId` and the rest of its group, or the latest event that isn't an undo if `null`. */
export async function undoEvent(eventId: number | null): Promise<UndoEventResponse | null> {
  return apiFetch('Undo', '/api/undo', {
    method: 'POST',
    body: JSON.stringify({ event_id: eventId, undo_group: true }),
  });
}

/** This server's two-way sync state (`GET /api/sync/status`). */
export interface SyncStatus {
  role: 'client' | 'server';
  replica: number;
  sync_url: string | null;
  sync_api_key_set: boolean;
  usn: number | null;
  last_sync_at: number | null;
  needs_full_sync: string | null;
  conflicts: number;
}

export interface SyncReport {
  mode: 'normal' | 'full_upload' | 'full_download' | null;
  pulled: number;
  pushed: number;
  conflicts: number[];
  notes_changed: number[];
  notes_deleted: number[];
}

export async function getSyncStatus(): Promise<SyncStatus> {
  return apiFetch('Sync status', '/api/sync/status');
}

/** Syncs this (laptop) server with its `sync_url`. A full sync is left to `spares sync cloud`. */
export async function runSync(): Promise<SyncReport> {
  return apiFetch('Sync', '/api/sync/run', {
    method: 'POST',
    body: JSON.stringify({ mode: 'normal' }),
  });
}

/** What an undo or redo reversed, e.g. `Undone: Rate card 3 (Good)`. */
export function describeReverted(verb: 'Undone' | 'Redone', events: EventSummary[]): string {
  return `${verb}: ${events.map(e => e.description).join('; ')}`;
}

/**
 * Redoes the undo event `eventId` and the rest of its group, or if `null`, the latest undo unless
 * an action was taken after it.
 */
export async function redoEvent(eventId: number | null): Promise<RedoEventResponse | null> {
  return apiFetch('Redo', '/api/redo', {
    method: 'POST',
    body: JSON.stringify({ event_id: eventId, redo_group: true }),
  });
}

export async function getStatistics(schedulerName: string, date: Date = new Date()): Promise<StatisticsResponse> {
  return apiFetch('Statistics fetch', '/api/review/statistics', {
    method: 'POST',
    body: JSON.stringify({ scheduler_name: schedulerName, date: date.toISOString() }),
  });
}

/** `getStatistics` for today, remembered for `cachedTodayStatistics`. */
export async function getTodayStatistics(schedulerName: string): Promise<StatisticsResponse> {
  const statistics = await getStatistics(schedulerName);
  todayStatistics.set(schedulerName, statistics);
  return statistics;
}

/** The statistics `getTodayStatistics` last returned for the scheduler. */
export function cachedTodayStatistics(schedulerName: string): StatisticsResponse | undefined {
  return todayStatistics.get(schedulerName);
}

/** The number of reviews on each day, remembered for `cachedReviewHistory`. */
export async function getReviewHistory(): Promise<ReviewHistoryResponse> {
  reviewHistory = await apiFetch<ReviewHistoryResponse>('Review history fetch', '/api/review/history');
  return reviewHistory;
}

/** The history `getReviewHistory` last returned. */
export function cachedReviewHistory(): ReviewHistoryResponse | undefined {
  return reviewHistory;
}

export async function getSchedulerRatings(name: string): Promise<Rating[]> {
  return memoize(`ratings:${name}`, () => withOfflineCopy(`ratings:${name}`, () => apiFetch('Ratings fetch', `/api/scheduler/${encodeURIComponent(name)}/ratings`)));
}

/** The cards a review would show next, to review offline. */
export async function fetchReviewQueue(req: ReviewQueueRequest): Promise<ReviewQueueResponse> {
  return apiFetch('Review queue fetch', '/api/review/queue', { method: 'POST', body: JSON.stringify(req) });
}

/** Submits actions performed offline. */
export async function syncOfflineActions(req: SyncOfflineActionsRequest): Promise<SyncOfflineActionsResponse> {
  return apiFetch('Sync', '/api/review/sync', { method: 'POST', body: JSON.stringify(req) });
}

export async function submitAction(req: SubmitStudyActionRequest): Promise<SubmitStudyActionResponse> {
  return apiFetch('Submit', '/api/review/submit', { method: 'POST', body: JSON.stringify(req) });
}

/** Reviews `count` cards ahead of time, like `spares card advance`. */
export async function advanceCards(schedulerName: string, count: number, query: string | null): Promise<number | null> {
  const res = await submitAction({ scheduler_name: schedulerName, action: { Advance: { count, query } } });
  return res.event_id;
}

/** Delays `count` reviews, like `spares card postpone`. */
export async function postponeCards(schedulerName: string, count: number, query: string | null): Promise<number | null> {
  const res = await submitAction({ scheduler_name: schedulerName, action: { Postpone: { count, query } } });
  return res.event_id;
}

// Notes

export async function searchNotes(query: string): Promise<NoteResponse[]> {
  const data = await apiFetch<{ Notes: [NoteResponse, string][] }>('Search', '/api/notes/search', {
    method: 'POST',
    body: JSON.stringify({ query, output_type: 'Notes' }),
  });
  return data.Notes.map(([note]) => note);
}

export async function listNotes(page: number, limit: number): Promise<NoteResponse[]> {
  return cacheListPage(`notes?${pageQuery(page, limit)}`, apiFetch('Notes fetch', `/api/notes?${pageQuery(page, limit)}`));
}

/** The page `listNotes` last returned, if it hasn't been invalidated since. */
export function cachedNotesPage(page: number, limit: number): NoteResponse[] | undefined {
  return listPages.get(`notes?${pageQuery(page, limit)}`) as NoteResponse[] | undefined;
}

export async function updateNote(id: number, data: string, tags: string[], keywords: string[]): Promise<NoteResponse> {
  const body = await apiFetch<{ notes: NoteResponse[]; event_id: number | null }>('Update', '/api/notes', {
    method: 'PATCH',
    body: JSON.stringify({
      selector: { Ids: [id] },
      data,
      keywords,
      tags: { SetTags: tags },
    }),
  });
  return body.notes[0];
}

export async function createNotes(parserId: number, requests: CreateNoteRequest[]): Promise<NoteResponse[]> {
  const body = await apiFetch<{ notes: NoteResponse[] }>('Create notes', '/api/notes', {
    method: 'POST',
    body: JSON.stringify({ parser_id: parserId, requests }),
  });
  return body.notes;
}

export async function deleteNotes(selector: NotesSelector): Promise<void> {
  await apiSend('Delete notes', '/api/notes', { method: 'DELETE', body: JSON.stringify({ selector }) });
}

// Cards

export async function listCards(page: number, limit: number): Promise<CardResponse[]> {
  return cacheListPage(`cards?${pageQuery(page, limit)}`, apiFetch('Cards fetch', `/api/cards?${pageQuery(page, limit)}`));
}

/** The page `listCards` last returned, if it hasn't been invalidated since. */
export function cachedCardsPage(page: number, limit: number): CardResponse[] | undefined {
  return listPages.get(`cards?${pageQuery(page, limit)}`) as CardResponse[] | undefined;
}

export async function getCard(id: number): Promise<CardResponse> {
  return apiFetch('Card fetch', `/api/cards/${id}`);
}

/** The card's review log, oldest first, including forgets. */
export async function getCardReviews(id: number): Promise<CardReviewEntry[]> {
  return apiFetch('Card reviews fetch', `/api/cards/${id}/reviews`);
}

export async function searchCards(query: string): Promise<CardResponse[]> {
  const data = await apiFetch<{ Cards: [CardResponse, string][] }>('Search', '/api/notes/search', {
    method: 'POST',
    body: JSON.stringify({ query, output_type: 'Cards' }),
  });
  return data.Cards.map(([card]) => card);
}

/** Cards that are frequently forgotten. */
export async function getLeeches(schedulerName: string): Promise<CardResponse[]> {
  return apiFetch('Leeches fetch', '/api/cards/leeches', {
    method: 'POST',
    body: JSON.stringify({ scheduler_name: schedulerName }),
  });
}

/** Unburies all buried cards, or only those matching `query`. */
export async function unburyCards(query: string | null): Promise<void> {
  await apiSend('Unbury', '/api/cards/unbury', { method: 'POST', body: JSON.stringify({ query }) });
}

// Tags

export async function listTags(page: number, limit: number): Promise<TagResponse[]> {
  return apiFetch('Tags fetch', `/api/tags?${pageQuery(page, limit)}`);
}

export async function getTag(id: number): Promise<TagResponse> {
  return apiFetch('Tag fetch', `/api/tags/${id}`);
}

export async function createTag(req: CreateTagRequest): Promise<TagResponse> {
  return apiFetch('Create tag', '/api/tags', { method: 'POST', body: JSON.stringify(req) });
}

export async function updateTag(req: UpdateTagRequest): Promise<TagResponse> {
  return apiFetch('Update tag', '/api/tags', { method: 'PATCH', body: JSON.stringify(req) });
}

export async function deleteTag(id: number): Promise<void> {
  await apiSend('Delete tag', `/api/tags/${id}`, { method: 'DELETE' });
}

/** Rebuilds a filtered tag's membership from its query. */
export async function rebuildTag(id: number): Promise<void> {
  await apiSend('Rebuild tag', `/api/tags/${id}/rebuild`);
}

// Parsers

export async function listParsers(page: number, limit: number): Promise<ParserResponse[]> {
  return apiFetch('Parsers fetch', `/api/parsers?${pageQuery(page, limit)}`);
}

export async function createParser(name: string): Promise<ParserResponse> {
  return apiFetch('Create parser', '/api/parsers', { method: 'POST', body: JSON.stringify({ name }) });
}

export async function updateParser(id: number, name: string): Promise<ParserResponse> {
  return apiFetch('Update parser', `/api/parsers/${id}`, { method: 'PATCH', body: JSON.stringify({ name }) });
}

export async function deleteParser(id: number): Promise<void> {
  await apiSend('Delete parser', `/api/parsers/${id}`, { method: 'DELETE' });
}

// Keywords and links

/** Every note keyword, as `[note id, keyword]`. */
export async function listKeywords(): Promise<[number, string][]> {
  return apiFetch('Keywords fetch', '/api/notes/keywords');
}

/** Notes whose keywords match `keyword`, best match first. */
export async function searchKeyword(keyword: string): Promise<MatchedKeywordResponse[]> {
  return apiFetch('Keyword search', '/api/notes/search/keyword', {
    method: 'POST',
    body: JSON.stringify({ keyword }),
  });
}

/** Keywords that notes link to but no note has. */
export async function getUnmatchedKeywords(): Promise<UnmatchedKeywordResponse[]> {
  return apiFetch('Unmatched keywords fetch', '/api/notes/unmatched-keywords');
}

/** Keywords on more than one note, as `[keyword, note ids]`. */
export async function getDuplicateKeywords(): Promise<[string, number[]][]> {
  return apiFetch('Duplicate keywords fetch', '/api/notes/duplicate-keywords');
}

/** Note links whose match score is below `scoreThreshold`. */
export async function getNoteLinks(scoreThreshold: number): Promise<NoteLink[]> {
  return apiFetch('Note links fetch', '/api/notes/search/note-links', {
    method: 'POST',
    body: JSON.stringify({ score_threshold: scoreThreshold }),
  });
}

// Offline copies made before note ids became letters (`spares migrate ids`) point at rendered files
// that were renamed, so they are dropped once. Actions yet to be synced are kept: they refer to
// cards, whose ids did not change.
const OFFLINE_FORMAT_KEY = 'spares_offline_format';
const OFFLINE_FORMAT = 'letter-note-ids';
if (localStorage.getItem(OFFLINE_FORMAT_KEY) !== OFFLINE_FORMAT) {
  clearOfflineCopies()
    .then(() => localStorage.setItem(OFFLINE_FORMAT_KEY, OFFLINE_FORMAT))
    .catch(console.error);
}
