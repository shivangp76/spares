import { useEffect } from "react";
import { parseNoteId } from "./utils";

/** An entry of the command palette (`CommandPalette`). */
export interface Command {
  /** Unique among the palette's entries. */
  id: string;
  label: string;
  /** Said on the right, e.g. the keys that do the same, or what kind of entry it is. */
  hint?: string;
  run: () => void;
}

/**
 * How well `query` matches `label`, or null if it doesn't: each of its characters in order, as
 * `rv` matches "Review". Higher is better: matches at the start of the label or of a word, and
 * runs of matched characters, count more, and a shorter label wins a tie.
 */
export function fuzzyScore(label: string, query: string): number | null {
  const text = label.toLowerCase();
  const q = query.trim().toLowerCase();
  if (q === "") return 0;
  let score = 0;
  let from = 0;
  let previous = -2;
  for (const char of q) {
    if (char === " ") continue;
    const at = text.indexOf(char, from);
    if (at === -1) return null;
    if (at === 0) score += 8;
    else if (/[\s:/(-]/.test(text[at - 1])) score += 5;
    if (at === previous + 1) score += 3;
    score += 1;
    previous = at;
    from = at + 1;
  }
  return score - text.length / 100;
}

/** `commands` that match `query`, best first, keeping their order on a tie. */
export function matchCommands(commands: Command[], query: string): Command[] {
  return commands
    .map((command, index) => ({
      command,
      index,
      score: fuzzyScore(command.label, query),
    }))
    .filter((m): m is typeof m & { score: number } => m.score !== null)
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map((m) => m.command);
}

/**
 * Whether `query` starts a word of `label`, as "stat" does "Go to Statistics", so the command it
 * names goes above the searches for the text itself.
 */
export function startsAWord(label: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return false;
  const text = label.toLowerCase();
  return (
    text.startsWith(q) ||
    text.split(/[\s:/(-]+/).some((word) => word.startsWith(q))
  );
}

/** Whether `query` could be a note's id, as `gir`, which the palette offers to open. */
export function looksLikeNoteId(query: string): boolean {
  const q = query.trim();
  return /^[a-z]{1,6}$/i.test(q) && parseNoteId(q) !== null;
}

let pageCommands: Command[] = [];

/** The commands the page shown offers the palette (`usePageCommands`). */
export function currentPageCommands(): Command[] {
  return pageCommands;
}

/** Offers `commands` to the command palette while the calling component is shown. */
export function usePageCommands(commands: Command[]): void {
  useEffect(() => {
    pageCommands = commands;
    return () => {
      pageCommands = [];
    };
  });
}
