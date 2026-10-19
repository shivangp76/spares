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
  NoteMatch,
  NoteRenderResponse,
  NoteResponse,
  NotesSelector,
  ParserResponse,
  QueryExample,
  Rating,
  RedoEventResponse,
  ReviewConfig,
  ReviewFilter,
  ReviewHistoryResponse,
  ReviewQueueRequest,
  ReviewQueueResponse,
  ReviewSnapshotResponse,
  SavedSearch,
  SearchField,
  SearchPage,
  StatisticsGraphsRequest,
  StatisticsGraphsResponse,
  StatisticsResponse,
  SubmitStudyActionRequest,
  SubmitStudyActionResponse,
  SyncOfflineActionsRequest,
  SyncOfflineActionsResponse,
  SyntaxExample,
  TagResponse,
  UndoEventResponse,
  UnmatchedKeywordResponse,
  UpdateCardsRequest,
  UpdateCardsResponse,
  UpdateTagRequest,
} from '../types/spares';
import { clearOfflineQueues, clearStatisticsEntries } from '../offline/db';

const STORAGE_KEY = 'spares_credentials';

export function getCredentials(): Credentials {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) throw new Error('Not authenticated');
  return JSON.parse(raw) as Credentials;
}

/**
 * The schedulers of the server at `serverUrl`, asked for with `apiKey`, to check both before they
 * are saved. Throws `NetworkError` if the server can't be reached, and an `ApiError` saying so if it
 * rejects the key.
 */
export async function getSchedulerNames({ serverUrl, apiKey }: Omit<Credentials, 'schedulerName'>): Promise<string[]> {
  const label = 'Log in';
  const res = await send(label, `${serverUrl}/api/schedulers`, { headers: { Authorization: `Bearer ${apiKey}` } });
  if (res.status === 401) throw new ApiError('The server rejected this API key.');
  await throwIfNotOk(res, label);
  return res.json() as Promise<string[]>;
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
  await clearStatisticsEntries();
}

/** An error the server returned. `span` is where in a search query the problem is, if it is one. */
export class ApiError extends Error {
  span: [number, number] | null;

  constructor(message: string, span: [number, number] | null = null) {
    super(message);
    this.span = span;
  }
}

/**
 * Throws with the server's `{ message }` (or plain-text body) when present, falling back to the
 * status code. The server's message is shown as it is, since it already says what went wrong.
 */
async function throwIfNotOk(res: Response, label: string): Promise<void> {
  if (res.ok) return;
  let message = `${label} failed: ${res.status}`;
  let span: [number, number] | null = null;
  const text = await res.text().catch(() => '');
  try {
    const body = JSON.parse(text) as { message?: unknown; span?: unknown };
    if (typeof body.message === 'string') message = body.message;
    if (Array.isArray(body.span) && body.span.length === 2 && body.span.every(n => typeof n === 'number')) {
      span = body.span as [number, number];
    }
  } catch {
    // Not JSON, e.g. a request the server rejected before reaching a handler
    if (text) message = text;
  }
  throw new ApiError(message, span);
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
  return (await modifyNoteTags({ Ids: [noteId] }, [tag], [])).eventId;
}

/** Adds and removes tags of the notes `selector` picks, leaving their other tags unchanged. */
export async function modifyNoteTags(
  selector: NotesSelector,
  tagsToAdd: string[],
  tagsToRemove: string[],
): Promise<{ notes: NoteResponse[]; eventId: number | null }> {
  const body = await apiFetch<{ notes: NoteResponse[]; event_id: number | null }>('Tag notes', '/api/notes', {
    method: 'PATCH',
    body: JSON.stringify({
      selector,
      tags: { ModifyTags: { tags_to_remove: tagsToRemove.length ? tagsToRemove : null, tags_to_add: tagsToAdd.length ? tagsToAdd : null } },
    }),
  });
  return { notes: body.notes, eventId: body.event_id };
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

/** The browser's IANA time zone, so the server counts the user's days even when it is elsewhere. */
export function localTimeZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

export async function getStatistics(schedulerName: string, date: Date = new Date()): Promise<StatisticsResponse> {
  return apiFetch('Statistics fetch', '/api/review/statistics', {
    method: 'POST',
    body: JSON.stringify({ scheduler_name: schedulerName, date: date.toISOString(), tz: localTimeZone() }),
  });
}

/** The statistics page's graphs, narrowed to `query` and `periodDays` (`null` for all time). */
export async function getStatisticsGraphs(
  schedulerName: string,
  { query, periodDays, forecastDays, date = new Date() }: { query: string | null; periodDays: number | null; forecastDays: number; date?: Date },
): Promise<StatisticsGraphsResponse> {
  const request: StatisticsGraphsRequest = {
    scheduler_name: schedulerName,
    query,
    period_days: periodDays,
    forecast_days: forecastDays,
    date: date.toISOString(),
    tz: localTimeZone(),
  };
  return apiFetch('Statistics graphs fetch', '/api/statistics/graphs', { method: 'POST', body: JSON.stringify(request) });
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

/** The number of reviews on each day, of the cards matching `query` if given. Without one, remembered for `cachedReviewHistory`. */
export async function getReviewHistory(query: string | null = null): Promise<ReviewHistoryResponse> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  const tz = localTimeZone();
  if (tz) params.set('tz', tz);
  const history = await apiFetch<ReviewHistoryResponse>('Review history fetch', `/api/review/history?${params}`);
  if (!query) reviewHistory = history;
  return history;
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

/** One page of the notes a search finds, with where each one's text matches. */
export async function searchNotesPage(query: string, page: number, limit: number): Promise<SearchPage<NoteMatch>> {
  const data = await apiFetch<{ total: number; items: { Notes: NoteMatch[] } }>('Search', '/api/notes/search/page', {
    method: 'POST',
    body: JSON.stringify({ query, output_type: 'Notes', page, limit }),
  });
  return { total: data.total, items: data.items.Notes };
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

/** One page of the cards a search finds. */
export async function searchCardsPage(query: string, page: number, limit: number): Promise<SearchPage<CardResponse>> {
  const data = await apiFetch<{ total: number; items: { Cards: [CardResponse, string][] } }>('Search', '/api/notes/search/page', {
    method: 'POST',
    body: JSON.stringify({ query, output_type: 'Cards', page, limit }),
  });
  return { total: data.total, items: data.items.Cards.map(([card]) => card) };
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

export async function listSavedSearches(): Promise<SavedSearch[]> {
  return apiFetch('Saved searches fetch', '/api/saved-searches');
}

/** Saves `query` as `name`, replacing the search saved with that name. */
export async function saveSearch(name: string, query: string): Promise<SavedSearch> {
  return apiFetch('Save search', '/api/saved-searches', { method: 'PUT', body: JSON.stringify({ name, query }) });
}

export async function deleteSavedSearch(name: string): Promise<void> {
  await apiSend('Delete saved search', `/api/saved-searches/${encodeURIComponent(name)}`, { method: 'DELETE' });
}

let searchFields: Promise<SearchField[]> | null = null;

/** The fields a search query can use, fetched once. */
export function getSearchFields(): Promise<SearchField[]> {
  searchFields ??= apiFetch<SearchField[]>('Search fields fetch', '/api/search/fields').catch(e => {
    searchFields = null;
    throw e;
  });
  return searchFields;
}

let searchExamples: Promise<QueryExample[]> | null = null;

/** Example queries, each with what it finds, fetched once. */
export function getSearchExamples(): Promise<QueryExample[]> {
  searchExamples ??= apiFetch<QueryExample[]>('Search examples fetch', '/api/search/examples').catch(e => {
    searchExamples = null;
    throw e;
  });
  return searchExamples;
}

// Tags

/** The name of every tag, for completing them in a search. */
export async function listTagNames(): Promise<string[]> {
  return (await listTags(1, 1_000_000)).map(t => t.name);
}

/** The name of every tag that isn't filtered, so can be added to and removed from notes by hand. */
export async function listManualTagNames(): Promise<string[]> {
  return (await listTags(1, 1_000_000)).filter(t => t.query === null).map(t => t.name);
}

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

/** How notes are written for the parser, with the cards each example makes, fetched once. */
export function getParserSyntax(id: number): Promise<SyntaxExample[]> {
  return memoize(`parserSyntax:${id}`, () => apiFetch('Parser syntax fetch', `/api/parsers/${id}/syntax`));
}

/** The names of the parsers spares has, which a parser here must have to be used. */
export function getKnownParserNames(): Promise<string[]> {
  return memoize('knownParsers', () => apiFetch('Parser names fetch', '/api/parsers/known'));
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
