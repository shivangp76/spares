import type {
  Credentials,
  GetReviewCardResponse,
  NoteResponse,
  Rating,
  StatisticsResponse,
  SubmitStudyActionRequest,
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

export function fileUrl(relativePath: string): string {
  const { serverUrl } = getCredentials();
  return `${serverUrl.replace(/\/$/, '')}/files/${relativePath}`;
}

export async function postReview(filter?: string): Promise<GetReviewCardResponse | null> {
  const { serverUrl } = getCredentials();
  const body = filter ? { filter: { Query: filter } } : {};
  const res = await fetch(`${serverUrl}/api/review`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`Review fetch failed: ${res.status}`);
  return res.json() as Promise<GetReviewCardResponse | null>;
}

export async function getStatistics(schedulerName: string): Promise<StatisticsResponse> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/review/statistics`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ scheduler_name: schedulerName, date: new Date().toISOString() }),
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

export async function submitAction(req: SubmitStudyActionRequest): Promise<void> {
  const { serverUrl } = getCredentials();
  const res = await fetch(`${serverUrl}/api/review/submit`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify(req),
  });
  if (!res.ok) throw new Error(`Submit failed: ${res.status}`);
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
