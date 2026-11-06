import type {
  CardMatch,
  CardResponse,
  CardReviewEntry,
  CardState,
  CreateNoteRequest,
  CreateTagRequest,
  Credentials,
  EventEntry,
  EventSummary,
  ForgetCardResponse,
  GetReviewCardResponse,
  ImageOcclusionData,
  ImportRequestOptions,
  ImportResponse,
  ImportSharedResponse,
  ShareSummary,
  ImageOcclusionEditorConfig,
  MatchedKeywordResponse,
  NoteGraphRequest,
  NoteGraphResponse,
  NoteLink,
  NoteMatch,
  NoteRenderResponse,
  NoteResponse,
  NotesSelector,
  OptimizeResponse,
  ParserResponse,
  QueryExample,
  QueuePositionRequest,
  QueuePositionResponse,
  Rating,
  RedoEventResponse,
  RescheduleResponse,
  ReviewConfig,
  ReviewFilter,
  ReviewHistoryResponse,
  ReviewSnapshotResponse,
  SavedSearch,
  SearchField,
  SearchPage,
  SearchSort,
  StatisticsGraphsRequest,
  StatisticsGraphsResponse,
  StatisticsResponse,
  StepsStatsRequest,
  StepsStatsResponse,
  SubmitStudyActionRequest,
  SubmitStudyActionResponse,
  SyntaxExample,
  TagResponse,
  UndoEventResponse,
  UnmatchedKeywordResponse,
  UpdateCardsRequest,
  UpdateCardsResponse,
  UpdateTagRequest,
} from "../types/spares";
import {
  COPY_FILES_CACHE,
  COPY_RENDER_ASSETS_CACHE,
  deviceMediaFile,
  localFetch,
  needsServer,
  usesLocalCopy,
} from "../local/transport";
import { reportServerReachable } from "./reachability";

const STORAGE_KEY = "spares_credentials";

export function getCredentials(): Credentials {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) throw new Error("Not authenticated");
  const credentials = JSON.parse(raw) as Partial<Credentials>;
  // Saved before accounts, with an API key instead of a session
  if (!credentials.serverUrl || !credentials.csrfToken)
    throw new Error("Not authenticated");
  return credentials as Credentials;
}

/**
 * Logs in to the host at `serverUrl`, which sets the session's cookie, and returns the session's
 * CSRF token and the account's name. Throws `NetworkError` if the host can't be reached, and an
 * `ApiError` if it refuses the name or password.
 */
export async function login(
  serverUrl: string,
  user: string,
  password: string,
): Promise<{ csrfToken: string; user: string }> {
  const label = "Log in";
  const res = await send(label, `${serverUrl}/api/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ user, password }),
  });
  if (res.status === 401) throw new ApiError("Wrong name or password.");
  await throwIfNotOk(res, label);
  const body = (await res.json()) as {
    csrf_token: string;
    user: { name: string };
  };
  return { csrfToken: body.csrf_token, user: body.user.name };
}

/** Ends the session on the host, then forgets the credentials. */
export async function logout(): Promise<void> {
  try {
    const { serverUrl } = getCredentials();
    await send("Log out", `${serverUrl}/api/auth/logout`, {
      method: "POST",
      headers: authHeaders(),
    });
  } catch {
    // Offline, or logged out already: the credentials go anyway.
  }
  clearCredentials();
}

/**
 * The schedulers of the host at `serverUrl`, asked for in the session just started, to check the
 * scheduler before it is saved. Throws `NetworkError` if the host can't be reached.
 */
export async function getSchedulerNames(serverUrl: string): Promise<string[]> {
  const label = "Log in";
  const res = await send(label, `${serverUrl}/api/schedulers`);
  await throwIfNotOk(res, label);
  return res.json() as Promise<string[]>;
}

export function saveCredentials(creds: Credentials): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(creds));
  credentialsChanged();
}

export function clearCredentials(): void {
  localStorage.removeItem(STORAGE_KEY);
  credentialsChanged();
}

const credentialsListeners = new Set<() => void>();

/** Calls `listener` after the credentials are saved or cleared, e.g. on logging in or out. */
export function onCredentialsChange(listener: () => void): () => void {
  credentialsListeners.add(listener);
  return () => credentialsListeners.delete(listener);
}

function credentialsChanged(): void {
  clearCaches();
  for (const listener of credentialsListeners) listener();
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

/** Forgets the responses kept in memory, e.g. after a sync changed what they show. */
export function clearCaches(): void {
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
    fetched.catch(() => {
      if (memoized.get(key) === fetched) memoized.delete(key);
    });
    memoized.set(key, fetched);
    result = fetched;
  }
  return result;
}

async function cacheListPage<T>(
  key: string,
  fetch: Promise<T[]>,
): Promise<T[]> {
  const page = await fetch;
  listPages.set(key, page);
  return page;
}

/** For a `FormData` body, without `Content-Type`, which the browser sets with the multipart boundary. */
function authHeaders(body?: RequestInit["body"]): HeadersInit {
  const { csrfToken } = getCredentials();
  if (body instanceof FormData) return { "X-CSRF-Token": csrfToken };
  if (body instanceof Blob)
    return {
      "X-CSRF-Token": csrfToken,
      "Content-Type": "application/octet-stream",
    };
  return {
    "X-CSRF-Token": csrfToken,
    "Content-Type": "application/json",
  };
}

/** The server could not be reached, e.g. because the device is offline. */
export class NetworkError extends Error {
  constructor(
    label: string,
    message = "failed: the server could not be reached",
  ) {
    super(`${label} ${message}`);
    this.name = "NetworkError";
  }
}

export function isNetworkError(e: unknown): boolean {
  return e instanceof NetworkError;
}

/** `fetch`, throwing a `NetworkError` if the server cannot be reached. */
async function send(
  label: string,
  url: string,
  init: RequestInit = {},
): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(url, init);
  } catch (e) {
    // An abort is the caller's, not the network's
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    reportServerReachable(false);
    throw new NetworkError(label);
  }
  reportServerReachable(true);
  return res;
}

/**
 * What this device's copy downloaded for `url`, if it uses the copy for the server logged in to.
 * The copy's files match its notes, which the server's newest renders may not until the next sync.
 */
async function copyFile(
  url: string,
  cacheName: string,
): Promise<Response | null> {
  if (
    typeof caches === "undefined" ||
    !usesLocalCopy(getCredentials().serverUrl)
  )
    return null;
  try {
    return (
      (await (await caches.open(cacheName)).match(url, { ignoreVary: true })) ??
      null
    );
  } catch {
    return null;
  }
}

/** A file from the server, which needs the session. Revalidated, so a newer render is shown. */
function sendForFile(label: string, url: string): Promise<Response> {
  return send(label, url, { headers: authHeaders(), cache: "no-cache" });
}

/**
 * An error the server returned. `span` is where in a search query the problem is, if it is one,
 * and `queryField` the field of the request that query is in, for a request with several, like
 * `match_query` of `getQueuePositions`.
 */
export class ApiError extends Error {
  span: [number, number] | null;
  queryField: string | null;

  constructor(
    message: string,
    span: [number, number] | null = null,
    queryField: string | null = null,
  ) {
    super(message);
    this.span = span;
    this.queryField = queryField;
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
  let queryField: string | null = null;
  const text = await res.text().catch(() => "");
  try {
    const body = JSON.parse(text) as {
      message?: unknown;
      span?: unknown;
      query_field?: unknown;
    };
    if (typeof body.message === "string") message = body.message;
    if (typeof body.query_field === "string") queryField = body.query_field;
    if (
      Array.isArray(body.span) &&
      body.span.length === 2 &&
      body.span.every((n) => typeof n === "number")
    ) {
      span = body.span as [number, number];
    }
    // The session ended: log in again.
    if (
      res.status === 401 &&
      (body as { unauthorized?: unknown }).unauthorized === true
    ) {
      clearCredentials();
      if (
        typeof window !== "undefined" &&
        window.location.pathname !== "/login"
      )
        window.location.assign("/login");
    }
  } catch {
    // Not JSON, e.g. a request the server rejected before reaching a handler
    if (text) message = text;
  }
  throw new ApiError(message, span, queryField);
}

async function apiFetch<T>(
  label: string,
  path: string,
  init: RequestInit = {},
): Promise<T> {
  const res = await apiSend(label, path, init);
  return res.json() as Promise<T>;
}

/**
 * Like `apiFetch`, for endpoints whose response body is empty. On a device that keeps a copy of
 * the collection, the copy answers, and only what it can't (e.g. rendering, uploads) goes to the
 * server.
 */
async function apiSend(
  label: string,
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const { serverUrl } = getCredentials();
  if ((init.method ?? "GET") !== "GET") listPages.clear();
  const request = { ...init, headers: authHeaders(init.body) };
  let res: Response | null = null;
  if (usesLocalCopy(serverUrl)) {
    res = await localFetch(path, request);
    if (await needsServer(res)) {
      try {
        res = await send(label, `${serverUrl}${path}`, request);
      } catch (e) {
        if (isNetworkError(e))
          throw new NetworkError(
            label,
            "needs the server, which could not be reached",
          );
        throw e;
      }
    }
  }
  res ??= await send(label, `${serverUrl}${path}`, request);
  await throwIfNotOk(res, label);
  return res;
}

function pageQuery(page: number, limit: number): string {
  return `page=${page}&limit=${limit}`;
}

/** A rendered file's URL on `serverUrl`, by default the server logged in to. */
export function fileUrl(
  relativePath: string,
  serverUrl = getCredentials().serverUrl,
): string {
  return `${serverUrl.replace(/\/$/, "")}/files/${relativePath}`;
}

/**
 * Downloads a rendered file, which needs the session like every other request. A device that keeps
 * a copy of the collection asks the server only for a file the copy hasn't downloaded.
 */
export async function fetchFile(relativePath: string): Promise<Blob> {
  const url = fileUrl(relativePath);
  const res =
    (await copyFile(url, COPY_FILES_CACHE)) ??
    (await sendForFile("Loading file", url));
  await throwIfNotOk(res, "Loading file");
  return res.blob();
}

/** Package specs (`namespace/name/version`) that browser-rendered sources import. */
export async function listRenderPackages(): Promise<string[]> {
  const { serverUrl } = getCredentials();
  if (usesLocalCopy(serverUrl))
    return apiFetch("Render packages fetch", "/api/render-assets/packages");
  const url = `${serverUrl}/api/render-assets/packages`;
  const res = await sendForFile("Render packages fetch", url);
  if (!res.ok) throw new Error(`Render packages fetch failed: ${res.status}`);
  return res.json();
}

/** Typst files that browser-rendered sources read, e.g. preambles, by the absolute paths the sources use. */
export async function listRenderSources(): Promise<string[]> {
  const { serverUrl } = getCredentials();
  if (usesLocalCopy(serverUrl))
    return apiFetch("Render sources fetch", "/api/render-assets/sources");
  const url = `${serverUrl}/api/render-assets/sources`;
  const res = await sendForFile("Render sources fetch", url);
  if (!res.ok) throw new Error(`Render sources fetch failed: ${res.status}`);
  return res.json();
}

/** A render asset's URL on `serverUrl`, by default the server logged in to. */
export function renderAssetUrl(
  path: string,
  serverUrl = getCredentials().serverUrl,
): string {
  return `${serverUrl}/api/render-assets?path=${encodeURIComponent(path)}`;
}

/**
 * A file a browser-rendered source reads, by the absolute path the source uses. `null` if the
 * server does not have it. A device that keeps a copy of the collection first looks among the
 * images and clips added on it, which the server doesn't have until the next sync, and the images of
 * cards' image occlusions, which it makes itself.
 */
export async function fetchRenderAsset(
  path: string,
): Promise<Uint8Array | null> {
  if (usesLocalCopy(getCredentials().serverUrl)) {
    // Its error, e.g. for an image occlusion whose image it can't read, names that image, which
    // the server's 404 for the file the device makes from it wouldn't
    const added = await deviceMediaFile(path);
    if (added) return added;
  }
  const url = renderAssetUrl(path);
  const res =
    (await copyFile(url, COPY_RENDER_ASSETS_CACHE)) ??
    (await sendForFile("Render asset fetch", url));
  if (res.status === 404) return null;
  if (!res.ok)
    throw new Error(`Render asset fetch failed for ${path}: ${res.status}`);
  return new Uint8Array(await res.arrayBuffer());
}

export async function getImageOcclusionEditorConfig(): Promise<ImageOcclusionEditorConfig> {
  return memoize("image-occlusion-editor-config", async () => {
    const res = await apiSend(
      "Image occlusion editor config",
      "/api/image-occlusions/editor-config",
    );
    return res.json();
  });
}

// Images, clips and image occlusions are stored on the server, or, on a device that keeps a copy
// of the collection, on the device until its next sync uploads them (see `spares_core::sync::media`).

/** Stores a new image occlusion. `snippet` is its block in the parser's syntax, ready to insert into a note. */
export async function createImageOcclusion(
  parserId: number,
  image: File,
  clozesSvg: string,
): Promise<{ image_occlusion: ImageOcclusionData; snippet: string }> {
  const form = new FormData();
  form.append("parser_id", String(parserId));
  form.append("image", image, image.name);
  form.append("clozes", clozesSvg);
  return apiFetch("Save image occlusion", "/api/image-occlusions", {
    method: "POST",
    body: form,
  });
}

/** Stores an image pasted or dropped into a note. `snippet` shows it in the parser's syntax, ready to insert into the note. */
export async function uploadImage(
  parserId: number,
  image: File,
): Promise<{ path: string; snippet: string }> {
  const form = new FormData();
  form.append("parser_id", String(parserId));
  form.append("image", image, image.name);
  return apiFetch(`Upload ${image.name}`, "/api/images", {
    method: "POST",
    body: form,
  });
}

/** Stores an audio clip recorded for a note. `snippet` links to it in the parser's syntax, ready to insert into the note. */
export async function uploadAudio(
  parserId: number,
  audio: File,
): Promise<{ path: string; snippet: string }> {
  const form = new FormData();
  form.append("parser_id", String(parserId));
  form.append("audio", audio, audio.name);
  return apiFetch("Upload recording", "/api/audio", {
    method: "POST",
    body: form,
  });
}

/** Has the server download and store an image dragged from a web page, which browsers hand over
    by URL only. Like `uploadImage`, `snippet` shows it in the parser's syntax. */
export async function uploadImageUrl(
  parserId: number,
  url: string,
): Promise<{ path: string; snippet: string }> {
  return apiFetch("Download image", "/api/images/url", {
    method: "POST",
    body: JSON.stringify({ parser_id: parserId, url }),
  });
}

/** A file of a stored image occlusion, by the absolute path a note refers to it by. */
export async function fetchImageOcclusionFile(path: string): Promise<Blob> {
  const res = await apiSend(
    "Image occlusion file",
    `/api/image-occlusions/file?path=${encodeURIComponent(path)}`,
  );
  return res.blob();
}

/**
 * The `order`th image occlusion (from 1) of card `cardOrder` of a note, without masks, as object
 * URLs to revoke: its original image, and its markup to show over it at the same width, if it has
 * any. On a device that keeps a copy, the copy answers.
 */
export async function fetchUnmaskedImageOcclusion(
  noteId: number,
  cardOrder: number,
  order: number,
): Promise<{ image: string; markup: string | null }> {
  const path = (part: "image" | "markup") =>
    `/api/review/unmasked-image-occlusion?note_id=${noteId}&card_order=${cardOrder}&order=${order}&part=${part}`;
  const [image, markup] = await Promise.all([
    apiSend("Image occlusion without masks", path("image")).then((res) =>
      res.blob(),
    ),
    apiSend("Image occlusion markup", path("markup")).then((res) =>
      res.status === 204 ? null : res.blob(),
    ),
  ]);
  return {
    image: URL.createObjectURL(image),
    markup: markup && URL.createObjectURL(markup),
  };
}

/** The note's image occlusions, in the order they appear in it. */
export async function listNoteImageOcclusions(
  noteId: number,
): Promise<ImageOcclusionData[]> {
  return apiFetch("Image occlusions", `/api/notes/${noteId}/image-occlusions`);
}

/** Replaces the clozes of the note's `index`th image occlusion (0 based). The server (or this
    device's copy) stores them as new files and updates the note, so its cards follow and the change
    can be undone. */
export async function updateNoteImageOcclusion(
  noteId: number,
  index: number,
  clozesSvg: string,
): Promise<NoteResponse> {
  const body = await apiFetch<{
    notes: NoteResponse[];
    event_id: number | null;
  }>("Save image occlusion", `/api/notes/${noteId}/image-occlusions/${index}`, {
    method: "PUT",
    body: JSON.stringify({ clozes: clozesSvg }),
  });
  return body.notes[0];
}

export async function postReview(
  filter?: ReviewFilter,
): Promise<GetReviewCardResponse | null> {
  return apiFetch("Review fetch", "/api/review", {
    method: "POST",
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/**
 * When a review with `filter` that has nothing to show now can show its next card: a learning card
 * due later today, once it comes within the learn ahead limit. `null` when the review is done for
 * the day.
 */
export async function nextReviewAvailableAt(
  filter?: ReviewFilter,
): Promise<string | null> {
  return apiFetch("Next review time fetch", "/api/review/next-available", {
    method: "POST",
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/** Fetches a specific card in review form, e.g. to refresh it after its note was edited. `null` if it no longer exists. */
export async function getReviewCardById(
  cardId: number,
  filter?: ReviewFilter,
): Promise<GetReviewCardResponse | null> {
  return apiFetch("Review card fetch", `/api/review/card/${cardId}`, {
    method: "POST",
    body: JSON.stringify(filter ? { filter } : {}),
  });
}

/** Resolves a query that uses `limit` into a filtered tag, built once per day. */
export async function createReviewSnapshot(
  query: string,
): Promise<ReviewSnapshotResponse> {
  return apiFetch("Review snapshot", "/api/review/snapshot", {
    method: "POST",
    body: JSON.stringify({ query }),
  });
}

export async function getReviewConfig(): Promise<ReviewConfig> {
  return memoize("reviewConfig", () =>
    apiFetch("Review config fetch", "/api/review/config"),
  );
}

export async function getTagByName(name: string): Promise<TagResponse> {
  return apiFetch("Tag fetch", `/api/tags/name/${encodeURIComponent(name)}`);
}

export async function getNote(id: number): Promise<NoteResponse> {
  return apiFetch("Note fetch", `/api/notes/${id}`);
}

/** Where to find the note's rendered file, or its source if it is compiled in the browser. */
export async function getNoteRender(id: number): Promise<NoteRenderResponse> {
  return apiFetch("Note render fetch", `/api/notes/${id}/render`);
}

export async function getCardsForNote(noteId: number): Promise<CardResponse[]> {
  return apiFetch("Cards fetch", `/api/cards/note_id/${noteId}`);
}

export async function updateCards(
  req: UpdateCardsRequest,
): Promise<UpdateCardsResponse> {
  return apiFetch("Card update", "/api/cards", {
    method: "PATCH",
    body: JSON.stringify(req),
  });
}

export async function forgetCard(cardId: number): Promise<ForgetCardResponse> {
  return apiFetch("Forget card", `/api/cards/${cardId}/forget`, {
    method: "POST",
  });
}

/** Adds a tag to a note, leaving its other tags unchanged. Returns the event id. */
export async function tagNote(
  noteId: number,
  tag: string,
): Promise<number | null> {
  return (await modifyNoteTags({ Ids: [noteId] }, [tag], [])).eventId;
}

/** Adds and removes tags of the notes `selector` picks, leaving their other tags unchanged. */
export async function modifyNoteTags(
  selector: NotesSelector,
  tagsToAdd: string[],
  tagsToRemove: string[],
): Promise<{ notes: NoteResponse[]; eventId: number | null }> {
  const body = await apiFetch<{
    notes: NoteResponse[];
    event_id: number | null;
  }>("Tag notes", "/api/notes", {
    method: "PATCH",
    body: JSON.stringify({
      selector,
      tags: {
        ModifyTags: {
          tags_to_remove: tagsToRemove.length ? tagsToRemove : null,
          tags_to_add: tagsToAdd.length ? tagsToAdd : null,
        },
      },
    }),
  });
  return { notes: body.notes, eventId: body.event_id };
}

/**
 * Regenerates a note's rendered files (and its cards' and linked notes'), as the CLI does after
 * syncing a note. Not on a device that keeps a copy, whose changes the server renders once a sync
 * brings them.
 */
export async function renderNote(noteId: number): Promise<void> {
  if (usesLocalCopy(getCredentials().serverUrl)) return;
  await apiSend("Render note", "/api/notes/generate_files", {
    method: "POST",
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
export async function undoEvent(
  eventId: number | null,
): Promise<UndoEventResponse | null> {
  return apiFetch("Undo", "/api/undo", {
    method: "POST",
    body: JSON.stringify({ event_id: eventId, undo_group: true }),
  });
}

/** A page of the history of changes, newest first, before the entry `before` (`GET /api/events`). Ratings only with `ratings`. */
export async function listEvents(
  before: number | null,
  ratings: boolean,
  limit = 30,
): Promise<EventEntry[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (before !== null) params.set("before", String(before));
  if (ratings) params.set("ratings", "true");
  return apiFetch("History fetch", `/api/events?${params}`);
}

/** A file spares saved before overwriting it in its notes repository, since it did not write it. */
export interface SavedCopy {
  /** Relative to `overwritten_dir`: the time it was saved, then its path in the repository */
  path: string;
  /** Whether the current file has the same contents, so nothing in the copy is lost */
  matches_current: boolean;
}

/** The notes repository's state (`GET /api/notes/vcs/status`), as far as the web UI uses it. */
export interface NotesVcsStatus {
  enabled: boolean;
  overwritten_dir: string | null;
  saved_copies: SavedCopy[];
}

export async function getNotesVcsStatus(): Promise<NotesVcsStatus> {
  return apiFetch("Notes repository status", "/api/notes/vcs/status");
}

/** Deletes the saved copies at `paths`, once the user checked nothing in them is lost. */
export async function removeSavedCopies(paths: string[]): Promise<void> {
  await apiSend("Delete saved copies", "/api/notes/vcs/saved-copies/remove", {
    method: "POST",
    body: JSON.stringify({ paths }),
  });
}

/** This host's two-way sync state (`GET /api/sync/status`). */
export interface SyncStatus {
  replica: number;
  /** The collection's upstream, which this host's copy syncs with; `null` on the root */
  sync_url: string | null;
  sync_token_set: boolean;
  usn: number | null;
  last_sync_at: number | null;
  needs_full_sync: string | null;
  conflicts: number;
}

export interface SyncReport {
  mode: "normal" | "full_upload" | "full_download" | null;
  pulled: number;
  pushed: number;
  conflicts: number[];
  notes_changed: number[];
  notes_deleted: number[];
}

export async function getSyncStatus(): Promise<SyncStatus> {
  return apiFetch("Sync status", "/api/sync/status");
}

/** Syncs this (laptop) server with its `sync_url`. A full sync is left to `spares sync cloud`. A device that keeps a copy syncs it with `syncLocalCopy` instead. */
export async function runSync(): Promise<SyncReport> {
  return apiFetch("Sync", "/api/sync/run", {
    method: "POST",
    body: JSON.stringify({ mode: "normal" }),
  });
}

/** A copy of the collection enrolled with the root of its sync chain under a replica number of its own (`GET /api/sync/devices`). */
export interface SyncDevice {
  replica: number;
  name: string;
  enrolled_at: number;
  /** Its last push, which a full upload keeps */
  last_sync_at: number | null;
}

/** The copies enrolled with the root of the collection's sync chain, which a host with an upstream asks it for. */
export async function listSyncDevices(): Promise<SyncDevice[]> {
  return apiFetch("Devices fetch", "/api/sync/devices");
}

/** Frees a device's replica number at the root, e.g. one whose browser deleted its copy. It can no longer sync. */
export async function removeSyncDevice(replica: number): Promise<void> {
  await apiSend("Remove device", "/api/sync/unenroll", {
    method: "POST",
    body: JSON.stringify({ replica }),
  });
}

/** A note a sync could not merge on its own (`spares_core::sync::conflicts`). Texts have card ids
    in place of card orders (`o:c40` is card 40), so the same card reads the same on both sides. */
export interface SyncConflict {
  id: number;
  note_id: number;
  /** `text`: both sides changed the same lines, and this side's text was kept. `deleted_there`:
      the other side deleted the note, which was kept. `deleted_here`: deleted here, so the note
      came back with the other side's text. */
  kind: "text" | "deleted_there" | "deleted_here";
  created_at: number;
  /** The text at the last sync, if known */
  base: string | null;
  /** The text now, `null` if the note has been deleted since */
  local: string | null;
  /** The other side's text, `null` if it deleted the note */
  remote: string | null;
}

/** Keep the note as it is, delete it, or set its text. */
export type ConflictResolution = "keep" | "delete" | { text: string };

/** The conflicts of this client (a device's copy, or the laptop's server). */
export async function listSyncConflicts(): Promise<SyncConflict[]> {
  return apiFetch("Conflicts fetch", "/api/sync/conflicts");
}

/** Resolves a conflict, as a change the next sync sends. */
export async function resolveSyncConflict(
  id: number,
  resolution: ConflictResolution,
): Promise<void> {
  await apiSend("Resolve conflict", `/api/sync/conflicts/${id}`, {
    method: "POST",
    body: JSON.stringify(resolution),
  });
}

/** What an undo or redo reversed, e.g. `Undone: Rate card 3 (Good)`. */
export function describeReverted(
  verb: "Undone" | "Redone",
  events: EventSummary[],
): string {
  return `${verb}: ${events.map((e) => e.description).join("; ")}`;
}

/**
 * Redoes the undo event `eventId` and the rest of its group, or if `null`, the latest undo unless
 * an action was taken after it.
 */
export async function redoEvent(
  eventId: number | null,
): Promise<RedoEventResponse | null> {
  return apiFetch("Redo", "/api/redo", {
    method: "POST",
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

export async function getStatistics(
  schedulerName: string,
  date: Date = new Date(),
): Promise<StatisticsResponse> {
  return apiFetch("Statistics fetch", "/api/review/statistics", {
    method: "POST",
    body: JSON.stringify({
      scheduler_name: schedulerName,
      date: date.toISOString(),
      tz: localTimeZone(),
    }),
  });
}

/** The statistics page's graphs, narrowed to `query` and `periodDays` (`null` for all time). */
export async function getStatisticsGraphs(
  schedulerName: string,
  {
    query,
    periodDays,
    forecastDays,
    date = new Date(),
  }: {
    query: string | null;
    periodDays: number | null;
    forecastDays: number;
    date?: Date;
  },
): Promise<StatisticsGraphsResponse> {
  const request: StatisticsGraphsRequest = {
    scheduler_name: schedulerName,
    query,
    period_days: periodDays,
    forecast_days: forecastDays,
    date: date.toISOString(),
    tz: localTimeZone(),
  };
  return apiFetch("Statistics graphs fetch", "/api/statistics/graphs", {
    method: "POST",
    body: JSON.stringify(request),
  });
}

/** How quickly a preset's cards are forgotten in their steps, and the steps that fit. */
export async function getStepsStats(
  request: StepsStatsRequest,
): Promise<StepsStatsResponse> {
  return apiFetch("Steps stats fetch", "/api/statistics/steps", {
    method: "POST",
    body: JSON.stringify(request),
  });
}

/**
 * Fits the scheduler's parameters to the review history, like `spares card optimize`. With
 * `apply`, saves them if they predict it better and reschedules every card. On the server, which
 * keeps the parameters in its `config.toml`; this can take a minute.
 */
export async function optimizeScheduler(
  schedulerName: string,
  apply: boolean,
): Promise<OptimizeResponse> {
  return apiFetch(
    "Optimize",
    `/api/scheduler/${encodeURIComponent(schedulerName)}/optimize`,
    { method: "POST", body: JSON.stringify({ apply }) },
  );
}

/**
 * Recomputes every card's memory state and due date from its review history, like `spares card
 * reschedule`. On the server, like `optimizeScheduler`.
 */
export async function rescheduleCards(
  schedulerName: string,
): Promise<RescheduleResponse> {
  return apiFetch(
    "Reschedule",
    `/api/scheduler/${encodeURIComponent(schedulerName)}/reschedule`,
    { method: "POST" },
  );
}

/** `getStatistics` for today, remembered for `cachedTodayStatistics`. */
export async function getTodayStatistics(
  schedulerName: string,
): Promise<StatisticsResponse> {
  const statistics = await getStatistics(schedulerName);
  todayStatistics.set(schedulerName, statistics);
  return statistics;
}

/** The statistics `getTodayStatistics` last returned for the scheduler. */
export function cachedTodayStatistics(
  schedulerName: string,
): StatisticsResponse | undefined {
  return todayStatistics.get(schedulerName);
}

/** The number of reviews on each day, of the cards matching `query` if given. Without one, remembered for `cachedReviewHistory`. */
export async function getReviewHistory(
  query: string | null = null,
): Promise<ReviewHistoryResponse> {
  const params = new URLSearchParams();
  if (query) params.set("query", query);
  const tz = localTimeZone();
  if (tz) params.set("tz", tz);
  const history = await apiFetch<ReviewHistoryResponse>(
    "Review history fetch",
    `/api/review/history?${params}`,
  );
  if (!query) reviewHistory = history;
  return history;
}

/** The history `getReviewHistory` last returned. */
export function cachedReviewHistory(): ReviewHistoryResponse | undefined {
  return reviewHistory;
}

export async function getSchedulerRatings(name: string): Promise<Rating[]> {
  return memoize(`ratings:${name}`, () =>
    apiFetch(
      "Ratings fetch",
      `/api/scheduler/${encodeURIComponent(name)}/ratings`,
    ),
  );
}

/** The states the scheduler puts cards in. */
export async function getSchedulerStates(name: string): Promise<CardState[]> {
  return memoize(`states:${name}`, () =>
    apiFetch(
      "States fetch",
      `/api/scheduler/${encodeURIComponent(name)}/states`,
    ),
  );
}

/** Where the cards matching a query are in a review's queue, and why the others are not in it. */
export async function getQueuePositions(
  req: QueuePositionRequest,
): Promise<QueuePositionResponse> {
  return apiFetch("Queue positions", "/api/review/queue/position", {
    method: "POST",
    body: JSON.stringify(req),
  });
}

export async function submitAction(
  req: SubmitStudyActionRequest,
): Promise<SubmitStudyActionResponse> {
  return apiFetch("Submit", "/api/review/submit", {
    method: "POST",
    body: JSON.stringify(req),
  });
}

/** Reviews `count` cards ahead of time, like `spares card advance`. */
export async function advanceCards(
  schedulerName: string,
  count: number,
  query: string | null,
): Promise<number | null> {
  const res = await submitAction({
    scheduler_name: schedulerName,
    action: { Advance: { count, query } },
  });
  return res.event_id;
}

/** Delays `count` reviews, like `spares card postpone`. */
export async function postponeCards(
  schedulerName: string,
  count: number,
  query: string | null,
): Promise<number | null> {
  const res = await submitAction({
    scheduler_name: schedulerName,
    action: { Postpone: { count, query } },
  });
  return res.event_id;
}

// Notes

export async function searchNotes(query: string): Promise<NoteResponse[]> {
  const data = await apiFetch<{ Notes: [NoteResponse, string][] }>(
    "Search",
    "/api/notes/search",
    {
      method: "POST",
      body: JSON.stringify({ query, output_type: "Notes" }),
    },
  );
  return data.Notes.map(([note]) => note);
}

/** One page of the notes a search finds, with where each one's text matches. */
export async function searchNotesPage(
  query: string,
  page: number,
  limit: number,
  sort: SearchSort | null = null,
): Promise<SearchPage<NoteMatch>> {
  const data = await apiFetch<{ total: number; items: { Notes: NoteMatch[] } }>(
    "Search",
    "/api/notes/search/page",
    {
      method: "POST",
      body: JSON.stringify({ query, output_type: "Notes", page, limit, sort }),
    },
  );
  return { total: data.total, items: data.items.Notes };
}

/** The notes a query finds, with the notes they link to, as the graph page shows them. */
export async function getNoteGraph(
  req: NoteGraphRequest,
): Promise<NoteGraphResponse> {
  return apiFetch("Graph", "/api/notes/graph", {
    method: "POST",
    body: JSON.stringify(req),
  });
}

export async function listNotes(
  page: number,
  limit: number,
): Promise<NoteResponse[]> {
  return cacheListPage(
    `notes?${pageQuery(page, limit)}`,
    apiFetch("Notes fetch", `/api/notes?${pageQuery(page, limit)}`),
  );
}

/** How many notes there are, for the pages `listNotes` has. */
export async function countNotes(): Promise<number> {
  return apiFetch("Notes count", "/api/notes/count");
}

/** The page `listNotes` last returned, if it hasn't been invalidated since. */
export function cachedNotesPage(
  page: number,
  limit: number,
): NoteResponse[] | undefined {
  return listPages.get(`notes?${pageQuery(page, limit)}`) as
    NoteResponse[] | undefined;
}

export async function updateNote(
  id: number,
  data: string,
  tags: string[],
  keywords: string[],
): Promise<NoteResponse> {
  const body = await apiFetch<{
    notes: NoteResponse[];
    event_id: number | null;
  }>("Update", "/api/notes", {
    method: "PATCH",
    body: JSON.stringify({
      selector: { Ids: [id] },
      data,
      keywords,
      tags: { SetTags: tags },
    }),
  });
  return body.notes[0];
}

export async function createNotes(
  parserId: number,
  requests: CreateNoteRequest[],
): Promise<NoteResponse[]> {
  const body = await apiFetch<{ notes: NoteResponse[] }>(
    "Create notes",
    "/api/notes",
    {
      method: "POST",
      body: JSON.stringify({ parser_id: parserId, requests }),
    },
  );
  return body.notes;
}

export async function deleteNotes(selector: NotesSelector): Promise<void> {
  await apiSend("Delete notes", "/api/notes", {
    method: "DELETE",
    body: JSON.stringify({ selector }),
  });
}

/** Changes the parser the notes `selector` picks are written for. Their text must be valid for it. */
export async function changeNotesParser(
  selector: NotesSelector,
  parserId: number,
): Promise<{ notes: NoteResponse[]; eventId: number | null }> {
  const body = await apiFetch<{
    notes: NoteResponse[];
    event_id: number | null;
  }>("Change parser", "/api/notes", {
    method: "PATCH",
    body: JSON.stringify({ selector, parser_id: parserId, tags: "None" }),
  });
  return { notes: body.notes, eventId: body.event_id };
}

/** The notes `query` finds, as files to import, keyed by their file extension, as `spares note export` writes them. */
export async function exportNotes(
  query: string,
): Promise<Record<string, string>> {
  const body: { files: Record<string, string> } = await apiFetch(
    "Export notes",
    "/api/notes/export",
    { method: "POST", body: JSON.stringify({ query }) },
  );
  return body.files;
}

/** What the notes `query` finds would be shared as (see `spares_core::api::note::export`). Always the server's: a device's copy may not have every file the notes name. */
export async function shareSummary(query: string): Promise<ShareSummary> {
  return apiFetch("Share summary", "/api/notes/share/summary", {
    method: "POST",
    body: JSON.stringify({ query }),
  });
}

/** The notes `query` finds, exported to share, as a zip of their files and media (see `spares_core::api::note::share_bundle`). */
export async function shareBundle(query: string): Promise<Blob> {
  const res = await apiSend("Export to share", "/api/notes/share/bundle", {
    method: "POST",
    body: JSON.stringify({ query }),
  });
  return res.blob();
}

/** Adds shared notes, from a file of them or a zip of them with their media, as new notes. On the server, since it stores the media. */
export async function importShared(file: File): Promise<ImportSharedResponse> {
  return apiFetch(
    "Import",
    `/api/notes/share/import?name=${encodeURIComponent(file.name)}`,
    { method: "POST", body: file },
  );
}

/**
 * Imports files of notes, or zips of them with their media, as `spares import` does: adding,
 * updating and deleting the notes they say to. On the server, which renders the notes and stores
 * the media. With `dry_run`, says what it would do and changes nothing.
 */
export async function importFiles(
  files: File[],
  options: ImportRequestOptions,
): Promise<ImportResponse> {
  const form = new FormData();
  form.append("options", JSON.stringify(options));
  for (const file of files) form.append("file", file, file.name);
  return apiFetch(
    options.dry_run ? "Import preview" : "Import",
    "/api/notes/import",
    {
      method: "POST",
      body: form,
    },
  );
}

// Cards

export async function listCards(
  page: number,
  limit: number,
): Promise<CardResponse[]> {
  return cacheListPage(
    `cards?${pageQuery(page, limit)}`,
    apiFetch("Cards fetch", `/api/cards?${pageQuery(page, limit)}`),
  );
}

/** How many cards there are, for the pages `listCards` has. */
export async function countCards(): Promise<number> {
  return apiFetch("Cards count", "/api/cards/count");
}

/** The page `listCards` last returned, if it hasn't been invalidated since. */
export function cachedCardsPage(
  page: number,
  limit: number,
): CardResponse[] | undefined {
  return listPages.get(`cards?${pageQuery(page, limit)}`) as
    CardResponse[] | undefined;
}

export async function getCard(id: number): Promise<CardResponse> {
  return apiFetch("Card fetch", `/api/cards/${id}`);
}

/** The card's review log, oldest first, including forgets. */
export async function getCardReviews(id: number): Promise<CardReviewEntry[]> {
  return apiFetch("Card reviews fetch", `/api/cards/${id}/reviews`);
}

export async function searchCards(query: string): Promise<CardResponse[]> {
  const data = await apiFetch<{ Cards: [CardResponse, string][] }>(
    "Search",
    "/api/notes/search",
    {
      method: "POST",
      body: JSON.stringify({ query, output_type: "Cards" }),
    },
  );
  return data.Cards.map(([card]) => card);
}

/** One page of the cards a search finds, sorted by `sort` first. An empty query finds every card. */
export async function searchCardsPage(
  query: string,
  page: number,
  limit: number,
  sort: SearchSort | null = null,
): Promise<SearchPage<CardMatch>> {
  const data = await apiFetch<{ total: number; items: { Cards: CardMatch[] } }>(
    "Search",
    "/api/notes/search/page",
    {
      method: "POST",
      body: JSON.stringify({ query, output_type: "Cards", page, limit, sort }),
    },
  );
  return { total: data.total, items: data.items.Cards };
}

/** Cards that are frequently forgotten. */
export async function getLeeches(
  schedulerName: string,
): Promise<CardResponse[]> {
  return apiFetch("Leeches fetch", "/api/cards/leeches", {
    method: "POST",
    body: JSON.stringify({ scheduler_name: schedulerName }),
  });
}

/** Unburies all buried cards, or only those matching `query`. */
export async function unburyCards(query: string | null): Promise<void> {
  await apiSend("Unbury", "/api/cards/unbury", {
    method: "POST",
    body: JSON.stringify({ query }),
  });
}

export async function listSavedSearches(): Promise<SavedSearch[]> {
  return apiFetch("Saved searches fetch", "/api/saved-searches");
}

/** Saves `query` as `name`, replacing the search saved with that name. */
export async function saveSearch(
  name: string,
  query: string,
): Promise<SavedSearch> {
  return apiFetch("Save search", "/api/saved-searches", {
    method: "PUT",
    body: JSON.stringify({ name, query }),
  });
}

export async function deleteSavedSearch(name: string): Promise<void> {
  await apiSend(
    "Delete saved search",
    `/api/saved-searches/${encodeURIComponent(name)}`,
    { method: "DELETE" },
  );
}

let searchFields: Promise<SearchField[]> | null = null;

/** The fields a search query can use, fetched once. */
export function getSearchFields(): Promise<SearchField[]> {
  searchFields ??= apiFetch<SearchField[]>(
    "Search fields fetch",
    "/api/search/fields",
  ).catch((e) => {
    searchFields = null;
    throw e;
  });
  return searchFields;
}

let searchExamples: Promise<QueryExample[]> | null = null;

/** Example queries, each with what it finds, fetched once. */
export function getSearchExamples(): Promise<QueryExample[]> {
  searchExamples ??= apiFetch<QueryExample[]>(
    "Search examples fetch",
    "/api/search/examples",
  ).catch((e) => {
    searchExamples = null;
    throw e;
  });
  return searchExamples;
}

// Tags

/** The name of every tag, for completing them in a search. */
export async function listTagNames(): Promise<string[]> {
  return (await listTags(1, 1_000_000)).map((t) => t.name);
}

/** The name of every tag that isn't filtered, so can be added to and removed from notes by hand. */
export async function listManualTagNames(): Promise<string[]> {
  return (await listTags(1, 1_000_000))
    .filter((t) => t.query === null)
    .map((t) => t.name);
}

export async function listTags(
  page: number,
  limit: number,
): Promise<TagResponse[]> {
  return apiFetch("Tags fetch", `/api/tags?${pageQuery(page, limit)}`);
}

export async function getTag(id: number): Promise<TagResponse> {
  return apiFetch("Tag fetch", `/api/tags/${id}`);
}

export async function createTag(req: CreateTagRequest): Promise<TagResponse> {
  return apiFetch("Create tag", "/api/tags", {
    method: "POST",
    body: JSON.stringify(req),
  });
}

export async function updateTag(req: UpdateTagRequest): Promise<TagResponse> {
  return apiFetch("Update tag", "/api/tags", {
    method: "PATCH",
    body: JSON.stringify(req),
  });
}

/** Renames `from` and the tags under it, as one action; moving a tag under another is renaming it. */
export async function renameTags(
  from: string,
  to: string,
): Promise<TagResponse[]> {
  return apiFetch("Rename tags", "/api/tags/rename", {
    method: "POST",
    body: JSON.stringify({ from, to }),
  });
}

export async function deleteTag(id: number): Promise<void> {
  await apiSend("Delete tag", `/api/tags/${id}`, { method: "DELETE" });
}

/** Rebuilds a filtered tag's membership from its query. */
export async function rebuildTag(id: number): Promise<void> {
  await apiSend("Rebuild tag", `/api/tags/${id}/rebuild`);
}

// Parsers

export async function listParsers(
  page: number,
  limit: number,
): Promise<ParserResponse[]> {
  return apiFetch("Parsers fetch", `/api/parsers?${pageQuery(page, limit)}`);
}

/** How notes are written for the parser, with the cards each example makes, fetched once. */
export function getParserSyntax(id: number): Promise<SyntaxExample[]> {
  return memoize(`parserSyntax:${id}`, () =>
    apiFetch("Parser syntax fetch", `/api/parsers/${id}/syntax`),
  );
}

/** The names of the parsers spares has, which a parser here must have to be used. */
export function getKnownParserNames(): Promise<string[]> {
  return memoize("knownParsers", () =>
    apiFetch("Parser names fetch", "/api/parsers/known"),
  );
}

export async function createParser(name: string): Promise<ParserResponse> {
  return apiFetch("Create parser", "/api/parsers", {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

export async function updateParser(
  id: number,
  name: string,
): Promise<ParserResponse> {
  return apiFetch("Update parser", `/api/parsers/${id}`, {
    method: "PATCH",
    body: JSON.stringify({ name }),
  });
}

export async function deleteParser(id: number): Promise<void> {
  await apiSend("Delete parser", `/api/parsers/${id}`, { method: "DELETE" });
}

// Keywords and links

/** Every note keyword, as `[note id, keyword]`. */
export async function listKeywords(): Promise<[number, string][]> {
  return apiFetch("Keywords fetch", "/api/notes/keywords");
}

/** Notes whose keywords match `keyword`, best match first. */
export async function searchKeyword(
  keyword: string,
): Promise<MatchedKeywordResponse[]> {
  return apiFetch("Keyword search", "/api/notes/search/keyword", {
    method: "POST",
    body: JSON.stringify({ keyword }),
  });
}

/** Keywords that notes link to but no note has. */
export async function getUnmatchedKeywords(): Promise<
  UnmatchedKeywordResponse[]
> {
  return apiFetch("Unmatched keywords fetch", "/api/notes/unmatched-keywords");
}

/** Keywords on more than one note, as `[keyword, note ids]`. */
export async function getDuplicateKeywords(): Promise<[string, number[]][]> {
  return apiFetch("Duplicate keywords fetch", "/api/notes/duplicate-keywords");
}

/** Note links whose match score is below `scoreThreshold`. */
export async function getNoteLinks(
  scoreThreshold: number,
): Promise<NoteLink[]> {
  return apiFetch("Note links fetch", "/api/notes/search/note-links", {
    method: "POST",
    body: JSON.stringify({ score_threshold: scoreThreshold }),
  });
}

// What browsers without a copy of the collection kept to review offline, before a device's copy
// replaced it: the downloaded queues, the ratings waiting to be sent and the statistics last
// shown (IndexedDB), the files those cards showed (Cache Storage), and responses such as the
// ratings (localStorage). Dropped once.
const RETIRED_OFFLINE_KEY = "spares_offline_retired";
if (localStorage.getItem(RETIRED_OFFLINE_KEY) === null) {
  (async () => {
    for (const key of Object.keys(localStorage)) {
      if (
        key.startsWith("spares_offline_copy:") ||
        key === "spares_offline_format"
      )
        localStorage.removeItem(key);
    }
    if (typeof caches !== "undefined") {
      await caches.delete("spares-offline-files");
      await caches.delete("spares-offline-render-assets");
    }
    if (typeof indexedDB !== "undefined")
      indexedDB.deleteDatabase("spares-offline");
    localStorage.setItem(RETIRED_OFFLINE_KEY, "1");
  })().catch(console.error);
}

/** Who can create an account on the host at `serverUrl`, and whether it sends email (for password resets). */
export interface SignupPolicy {
  signups: "closed" | "invite" | "open";
  email: boolean;
}

export async function getSignupPolicy(
  serverUrl: string,
): Promise<SignupPolicy> {
  const label = "Signup policy";
  const res = await send(label, `${serverUrl}/api/auth/signups`);
  await throwIfNotOk(res, label);
  return res.json() as Promise<SignupPolicy>;
}

/**
 * Creates an account. With an invite, it is logged in at once (the session's CSRF token is
 * returned); with open signups, a link to verify the email address is sent first (`verify`).
 */
export async function signUp(
  serverUrl: string,
  request: { name: string; email: string; password: string; invite?: string },
): Promise<
  { csrfToken: string; user: string } | { verify: true; message: string }
> {
  const label = "Sign up";
  const res = await send(label, `${serverUrl}/api/auth/signup`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      ...request,
      email: request.email || null,
      invite: request.invite || null,
    }),
  });
  await throwIfNotOk(res, label);
  const body = (await res.json()) as {
    csrf_token?: string;
    user?: { name: string };
    verify?: boolean;
    message?: string;
  };
  if (body.verify)
    return { verify: true, message: body.message ?? "Check your email." };
  return { csrfToken: body.csrf_token!, user: body.user!.name };
}

/** Emails a link to reset the password of the account with this address, if there is one. */
export async function requestPasswordReset(
  serverUrl: string,
  email: string,
): Promise<string> {
  const label = "Password reset";
  const res = await send(label, `${serverUrl}/api/auth/reset-request`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email }),
  });
  await throwIfNotOk(res, label);
  return ((await res.json()) as { message: string }).message;
}

/** Sets a new password with the token from a reset email. */
export async function resetPassword(
  serverUrl: string,
  token: string,
  password: string,
): Promise<void> {
  const label = "Password reset";
  const res = await send(label, `${serverUrl}/api/auth/reset`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token, password }),
  });
  await throwIfNotOk(res, label);
}

/** Changes the account's password; the account's other sessions end. */
export async function changePassword(
  current: string,
  newPassword: string,
): Promise<void> {
  await apiSend("Change password", "/api/auth/password", {
    method: "POST",
    body: JSON.stringify({ current, new: newPassword }),
  });
}

/** The account's whole collection, as a `.tar.gz` another host imports. */
export async function exportAccount(): Promise<Blob> {
  const { serverUrl } = getCredentials();
  const label = "Export";
  const res = await send(label, `${serverUrl}/api/account/export`, {
    headers: authHeaders(),
  });
  await throwIfNotOk(res, label);
  return res.blob();
}

/** A device token of the account's (`spares_server::host::auth::Device`): a `spares login`, or a host syncing with this one. Times are Unix seconds. */
export interface DeviceToken {
  id: number;
  user_id: string;
  name: string;
  /** `sync` only syncs: another host's copy of the collection */
  scope: "all" | "sync";
  /** The replica it enrolled as, if it keeps a copy */
  replica: number | null;
  created_at: number;
  last_used_at: number | null;
}

/** The account's device tokens that are not revoked. Always from the server, which keeps them. */
export async function listDeviceTokens(): Promise<DeviceToken[]> {
  const { serverUrl } = getCredentials();
  const label = "Devices fetch";
  const res = await send(label, `${serverUrl}/api/auth/devices`, {
    headers: authHeaders(),
  });
  await throwIfNotOk(res, label);
  return res.json() as Promise<DeviceToken[]>;
}

/** Revokes a device token, like `spares logout` on that device, and frees the replica it enrolled as. */
export async function revokeDeviceToken(id: number): Promise<void> {
  const { serverUrl } = getCredentials();
  const label = "Revoke device";
  const res = await send(label, `${serverUrl}/api/auth/devices/${id}`, {
    method: "DELETE",
    headers: authHeaders(),
  });
  await throwIfNotOk(res, label);
}

/** Deletes the account and its collection on the host. */
export async function deleteAccount(password: string): Promise<void> {
  await apiSend("Delete account", "/api/account", {
    method: "DELETE",
    body: JSON.stringify({ password }),
  });
  clearCredentials();
}
