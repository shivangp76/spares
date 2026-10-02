import type {
  CardResponse,
  CreateNoteRequest,
  CreateTagRequest,
  Credentials,
  ForgetCardResponse,
  GetReviewCardResponse,
  MatchedKeywordResponse,
  NoteLink,
  NoteRenderResponse,
  NoteResponse,
  NotesSelector,
  ParserResponse,
  Rating,
  ReviewConfig,
  ReviewFilter,
  ReviewSnapshotResponse,
  StatisticsResponse,
  SubmitStudyActionRequest,
  SubmitStudyActionResponse,
  TagResponse,
  UndoEventResponse,
  UnmatchedKeywordResponse,
  UpdateCardsRequest,
  UpdateCardsResponse,
  UpdateTagRequest,
} from '../types/spares';

const STORAGE_KEY = 'spares_credentials';

export function getCredentials(): Credentials {
  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) throw new Error('Not authenticated');
  return JSON.parse(raw) as Credentials;
}

export function saveCredentials(creds: Credentials): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(creds));
}

export function clearCredentials(): void {
  localStorage.removeItem(STORAGE_KEY);
}

function authHeaders(): HeadersInit {
  const { apiKey } = getCredentials();
  return {
    Authorization: `Bearer ${apiKey}`,
    'Content-Type': 'application/json',
  };
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
  const res = await fetch(`${serverUrl}${path}`, { ...init, headers: authHeaders() });
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

/** Package specs (`namespace/name/version`) that browser-rendered sources import. */
export async function listRenderPackages(): Promise<string[]> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/render-assets/packages`, { headers: authHeaders() });
  if (!res.ok) throw new Error(`Render packages fetch failed: ${res.status}`);
  return res.json();
}

/** A file a browser-rendered source reads, by the absolute path the source uses. `null` if the server does not have it. */
export async function fetchRenderAsset(path: string): Promise<Uint8Array | null> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/render-assets?path=${encodeURIComponent(path)}`, {
    headers: authHeaders(),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`Render asset fetch failed for ${path}: ${res.status}`);
  return new Uint8Array(await res.arrayBuffer());
}

export async function postReview(filter?: ReviewFilter): Promise<GetReviewCardResponse | null> {
  return apiFetch('Review fetch', '/api/review', {
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
  return apiFetch('Review config fetch', '/api/review/config');
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

/** Undoes `eventId` and the rest of its group, or the latest event if `null`. */
export async function undoEvent(eventId: number | null): Promise<UndoEventResponse | null> {
  return apiFetch('Undo', '/api/undo', {
    method: 'POST',
    body: JSON.stringify({ event_id: eventId, undo_group: true }),
  });
}

export async function getStatistics(schedulerName: string, date: Date = new Date()): Promise<StatisticsResponse> {
  return apiFetch('Statistics fetch', '/api/review/statistics', {
    method: 'POST',
    body: JSON.stringify({ scheduler_name: schedulerName, date: date.toISOString() }),
  });
}

export async function getSchedulerRatings(name: string): Promise<Rating[]> {
  return apiFetch('Ratings fetch', `/api/scheduler/${encodeURIComponent(name)}/ratings`);
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
  return apiFetch('Notes fetch', `/api/notes?${pageQuery(page, limit)}`);
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
  return apiFetch('Cards fetch', `/api/cards?${pageQuery(page, limit)}`);
}

export async function getCard(id: number): Promise<CardResponse> {
  return apiFetch('Card fetch', `/api/cards/${id}`);
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
