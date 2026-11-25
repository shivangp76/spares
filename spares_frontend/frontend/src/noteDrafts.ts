import { getLocalCopy } from "./local/transport";
import { getCredentials } from "./api/client";
import { useEffect, useState } from "react";

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

/** localStorage is already isolated by host origin. Capture the collection when an
 * editor opens, so a write from an old tab cannot acquire the next login's identity. */
export function draftStore(owner: string | null, collection = owner) {
  const prefix = JSON.stringify(["spares-drafts-v1", owner, collection]) + ":";
  function store<T>(key: string) {
    return {
      load: () => (owner === null ? null : load<T>(prefix + key)),
      save: (value: T | null) => {
        if (owner !== null) save(prefix + key, value);
      },
    };
  }
  return {
    newNoteDraft: store<NewNoteDraft>(NEW_NOTE_KEY),
    newNoteDefaults: store<NewNoteDefaults>(DEFAULTS_KEY),
    editDraft: (id: number) => store<EditDraft>(EDIT_PREFIX + id),
  };
}

export function useDraftStore() {
  const [store] = useState(() => {
    try {
      const credentials = getCredentials();
      // An older login predates collection IDs. Keep its drafts separate from collections identified by ID.
      const owner = credentials.accountId ?? "legacy-name:" + credentials.user;
      const copy = getLocalCopy();
      const collection = copy?.ready
        ? (copy.collectionId ?? "legacy-copy:" + copy.replica)
        : owner;
      return draftStore(owner, collection);
    } catch {
      return draftStore(null);
    }
  });
  return store;
}

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
