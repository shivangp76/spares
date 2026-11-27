/** The host's `/api/language-tools` (`spares_server/src/language_tools.rs`). */
import { apiFetch } from '../api/client';

export interface DictionaryEntry {
  expression: string;
  reading: string;
  glosses: string[];
  tags: string;
  source: string;
  matched: number;
  /** Innermost first, such as `["negative", "past"]` */
  inflections: string[];
  dictionary_id: number;
  dictionary: string;
  attribution: string;
  /** The notes with the word as a keyword */
  notes?: number[];
}

export interface LookupResponse {
  matched: number;
  language: string | null;
  entries: DictionaryEntry[];
}

export function lookUp(text: string, language: string | null): Promise<LookupResponse> {
  const query = new URLSearchParams({ text });
  if (language) query.set('language', language);
  return apiFetch('Dictionary lookup', `/api/language-tools/lookup?${query}`);
}
