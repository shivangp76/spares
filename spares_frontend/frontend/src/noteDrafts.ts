import { useEffect } from "react";

/**
 * Unsaved note text kept in this browser, so a reload, a closed tab or a crash doesn't lose it: the
 * new note being written, and each note's unsaved edits. Also what the last new note was written
 * with, to start the next one the same way.
 */

/** A note's text fields as the editors hold them, tags and keywords one per line. */
export interface NoteFields {
  data: string;
  tags: string;
  keywords: string;
}

export interface NewNoteDraft extends NoteFields {
  parserId: number | null;
  isSuspended: boolean;
}

export interface EditDraft extends NoteFields {
  /** The note's `updated_at` when the edits began, to tell whether it changed since. */
  base: string;
}

/** The parser and tags of the last note created here. */
export interface NewNoteDefaults {
  parserId: number;
  tags: string;
}

export interface Saved<T> {
  value: T;
  /** When it was saved, in milliseconds since the epoch. */
  savedAt: number;
}

const NEW_NOTE_KEY = "spares_new_note_draft";
const DEFAULTS_KEY = "spares_new_note_defaults";
const EDIT_PREFIX = "spares_note_draft_";
// Drafts older than this are dropped when next read
const MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

function load<T>(key: string): Saved<T> | null {
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return null;
    const saved = JSON.parse(raw) as Saved<T>;
    if (Date.now() - saved.savedAt > MAX_AGE_MS) {
      localStorage.removeItem(key);
      return null;
    }
    return saved;
  } catch {
    return null;
  }
}

function save<T>(key: string, value: T | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else
      localStorage.setItem(key, JSON.stringify({ value, savedAt: Date.now() }));
  } catch {
    // Storage full or blocked: the draft is only lost on reload, as before
  }
}

export const newNoteDraft = {
  load: () => load<NewNoteDraft>(NEW_NOTE_KEY),
  save: (draft: NewNoteDraft | null) => save(NEW_NOTE_KEY, draft),
};

export const editDraft = {
  load: (noteId: number) => load<EditDraft>(EDIT_PREFIX + noteId),
  save: (noteId: number, draft: EditDraft | null) =>
    save(EDIT_PREFIX + noteId, draft),
};

export const newNoteDefaults = {
  load: () => load<NewNoteDefaults>(DEFAULTS_KEY)?.value ?? null,
  save: (defaults: NewNoteDefaults) => save(DEFAULTS_KEY, defaults),
};

/**
 * Saves `value` with `write` whenever it changes, or clears it when null. Written at once rather
 * than after a pause, so closing the tab right after typing keeps the last keys; a note is small.
 */
export function useDraftSaver<T>(
  value: T | null,
  write: (value: T | null) => void,
  /** False leaves what is saved alone, e.g. while a draft is offered to restore. */
  enabled = true,
) {
  const json = JSON.stringify(value);
  useEffect(() => {
    if (enabled) write(JSON.parse(json) as T | null);
    // `write` is a stable store function; `json` stands for `value`
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [json, enabled]);
}

/** A time a draft was saved, as "10:42" today or "Nov 3, 10:42" before. */
export function draftTime(savedAt: number): string {
  const date = new Date(savedAt);
  const time = date.toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
  return date.toDateString() === new Date().toDateString()
    ? time
    : `${date.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}
