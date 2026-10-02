import type {
  CardResponse,
  Credentials,
  ForgetCardResponse,
  GetReviewCardResponse,
  NoteRenderResponse,
  NoteResponse,
  Rating,
  ReviewConfig,
  ReviewFilter,
  ReviewSnapshotResponse,
  StatisticsResponse,
  SubmitStudyActionRequest,
  SubmitStudyActionResponse,
  TagResponse,
  UndoEventResponse,
  UpdateCardsRequest,
  UpdateCardsResponse,
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

/** Throws with the server's `{ message }` when present, falling back to the status code. */
async function throwIfNotOk(res: Response, label: string): Promise<void> {
  if (res.ok) return;
  let message = `${label} failed: ${res.status}`;
  try {
    const body = await res.json() as { message?: unknown };
    if (typeof body.message === 'string') message = `${label} failed: ${body.message}`;
  } catch {
    // Not JSON; keep the status-code message
  }
  throw new Error(message);
}

async function apiFetch<T>(label: string, path: string, init: RequestInit = {}): Promise<T> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}${path}`, { ...init, headers: authHeaders() });
  await throwIfNotOk(res, label);
  return res.json() as Promise<T>;
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
  const { serverUrl } = getCredentials();
  // The response has an empty body, so it is not parsed
  const res = await fetch(`${serverUrl}/api/notes/generate_files`, {
    method: 'POST',
    headers: authHeaders(),
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
  await throwIfNotOk(res, 'Render note');
}

/** Undoes `eventId` and the rest of its group, or the latest event if `null`. */
export async function undoEvent(eventId: number | null): Promise<UndoEventResponse | null> {
  return apiFetch('Undo', '/api/undo', {
    method: 'POST',
    body: JSON.stringify({ event_id: eventId, undo_group: true }),
  });
}

export async function getStatistics(schedulerName: string, date: Date = new Date()): Promise<StatisticsResponse> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/review/statistics`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ scheduler_name: schedulerName, date: date.toISOString() }),
  });
  if (!res.ok) throw new Error(`Statistics fetch failed: ${res.status}`);
  return res.json();
}

export async function getSchedulerRatings(name: string): Promise<Rating[]> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/scheduler/${encodeURIComponent(name)}/ratings`, {
    headers: authHeaders(),
  });
  if (!res.ok) throw new Error(`Ratings fetch failed: ${res.status}`);
  return res.json();
}

export async function submitAction(req: SubmitStudyActionRequest): Promise<SubmitStudyActionResponse> {
  return apiFetch('Submit', '/api/review/submit', { method: 'POST', body: JSON.stringify(req) });
}

export async function searchNotes(query: string): Promise<NoteResponse[]> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/notes/search`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ query, output_type: 'Notes' }),
  });
  if (!res.ok) {
    const body = await res.text();
    let message = `Search failed: ${res.status}`;
    try {
      const parsed = JSON.parse(body);
      if (parsed.error) message = parsed.error;
      else if (typeof parsed === 'string') message = parsed;
    } catch {
      if (body) message = body;
    }
    throw new Error(message);
  }
  const data = await res.json() as { Notes: [NoteResponse, string][] };
  return data.Notes.map(([note]) => note);
}

export async function listNotes(page: number, limit: number): Promise<NoteResponse[]> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/notes?page=${page}&limit=${limit}`, {
    headers: authHeaders(),
  });
  if (!res.ok) throw new Error(`Notes fetch failed: ${res.status}`);
  return res.json();
}

export async function updateNote(id: number, data: string, tags: string[], keywords: string[]): Promise<NoteResponse> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/notes`, {
    method: 'PATCH',
    headers: authHeaders(),
    body: JSON.stringify({
      selector: { Ids: [id] },
      data,
      keywords,
      tags: { SetTags: tags },
    }),
  });
  if (!res.ok) throw new Error(`Update failed: ${res.status}`);
  const body = await res.json() as { notes: NoteResponse[]; event_id: number | null };
  return body.notes[0];
}
