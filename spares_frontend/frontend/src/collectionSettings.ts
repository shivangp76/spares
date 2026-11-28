/** What the Settings page's collection settings form edits, and how its values read and parse.
    The settings are documented on `spares_core::config::SparesExternalConfig`; the form has the
    common ones, and the TOML editor every one. */

import type { SettingValue } from "./types/spares";

export type SettingPath = string[];

/** The value at `path` of the settings' JSON, or `undefined`. */
export function valueAt(values: unknown, path: SettingPath): unknown {
  let value = values;
  for (const key of path) {
    if (value === null || typeof value !== "object") return undefined;
    value = (value as Record<string, unknown>)[key];
  }
  return value;
}

const UNITS: [string, number][] = [
  ["d", 86_400],
  ["h", 3_600],
  ["m", 60],
  ["s", 1],
];

/** A duration in seconds as the steps field shows it: `90s`, `10m`, `1h`, `2d`, `1h30m`. */
export function formatDuration(seconds: number): string {
  if (seconds === 0) return "0s";
  let left = seconds;
  let text = "";
  for (const [unit, size] of UNITS) {
    const count = Math.floor(left / size);
    if (count > 0) {
      text += `${count}${unit}`;
      left -= count * size;
    }
  }
  return text;
}

/** A duration such as `10m`, `1h30m` or `90` (seconds) in seconds, or `null` if it isn't one. */
export function parseDuration(text: string): number | null {
  const trimmed = text.trim().toLowerCase();
  if (/^\d+$/.test(trimmed)) return Number(trimmed);
  const parts = trimmed.match(/^(\d+(?:\.\d+)?[dhms])+$/);
  if (!parts) return null;
  let seconds = 0;
  for (const [, count, unit] of trimmed.matchAll(/(\d+(?:\.\d+)?)([dhms])/g)) {
    seconds += Number(count) * UNITS.find(([u]) => u === unit)![1];
  }
  return Math.round(seconds);
}

/** Steps as the steps field shows them: `1m 10m`. */
export function formatSteps(steps: number[]): string {
  return steps.map(formatDuration).join(" ");
}

/** Steps written as `1m 10m 1h` (or with commas), or an error saying which isn't a duration. */
export function parseSteps(text: string): number[] | string {
  const words = text.split(/[\s,]+/).filter(Boolean);
  const steps: number[] = [];
  for (const word of words) {
    const seconds = parseDuration(word);
    if (seconds === null || seconds <= 0)
      return `"${word}" is not a duration, such as 30s, 10m, 1h or 1d`;
    steps.push(seconds);
  }
  return steps;
}

/** The line an error from the host names, as in "the settings line 3: …". */
export function errorLine(message: string): number | null {
  const match = message.match(/\bline (\d+)\b/);
  return match ? Number(match[1]) : null;
}

/** How a field shows and parses its value. */
/** With `optional`, an empty input (or the select's first option) unsets the setting, so it takes
    its default, which for a preset's setting is the top-level one. */
export type FieldKind =
  | {
      type: "number";
      min?: number;
      max?: number;
      fraction?: boolean;
      optional?: boolean;
    }
  /** A number, or nothing for no limit (the setting's default) */
  | { type: "limit" }
  | { type: "bool" }
  | {
      type: "select";
      options: { value: unknown; label: string }[];
      /** The label of the first option, which unsets the setting */
      optional?: string;
    }
  | { type: "steps"; optional?: boolean }
  /** Seconds, shown in `unit`s */
  | { type: "duration"; unit: "minutes" | "days"; optional?: boolean }
  /** With `wide`, the input fills the row, as for a search */
  | { type: "text"; optional?: boolean; placeholder?: string; wide?: boolean }
  /** A date, `YYYY-MM-DD` */
  | { type: "date"; optional?: boolean }
  /** Tags separated by commas or spaces */
  | { type: "tags" }
  /** Named searches, one `name: query` a line */
  | { type: "parts" };

export interface FieldSpec {
  path: SettingPath;
  label: string;
  kind: FieldKind;
  hint?: string;
}

export interface FieldGroup {
  title: string;
  fields: FieldSpec[];
}

const REVIEW_ORDERS = [
  { value: "due", label: "Due date" },
  { value: "retrievability", label: "Most likely forgotten first" },
  {
    value: "relative_overdueness",
    label: "Most overdue for their interval first",
  },
  { value: "difficulty_descending", label: "Hardest first" },
  { value: "difficulty_ascending", label: "Easiest first" },
  { value: "random", label: "Random" },
  { value: "note_created", label: "Order the notes were created" },
];

const NEW_ORDERS = [
  { value: "created", label: "Order created" },
  { value: "newest_first", label: "Newest first" },
  { value: "random", label: "Random" },
  { value: "random_note", label: "Random, a whole note at a time" },
  { value: "unblocking", label: "Those other cards build on first" },
];

/** The form's settings. Every other one is in the TOML editor. */
export const FIELD_GROUPS: FieldGroup[] = [
  {
    title: "Daily limits",
    fields: [
      {
        path: ["new_cards_daily_limit"],
        label: "New cards a day",
        kind: { type: "number", min: 0 },
      },
      {
        path: ["reviews_daily_limit"],
        label: "Reviews a day",
        kind: { type: "limit" },
        hint: "Empty for no limit.",
      },
      {
        path: ["new_cards_ignore_review_limit"],
        label: "Show new cards even once the review limit is reached",
        kind: { type: "bool" },
      },
    ],
  },
  {
    title: "Days",
    fields: [
      {
        path: ["day_starts_at"],
        label: "A new day starts at",
        kind: {
          type: "select",
          options: Array.from({ length: 24 }, (_, hour) => ({
            value: hour,
            label: `${String(hour).padStart(2, "0")}:00`,
          })),
        },
      },
      {
        path: ["time_zone"],
        label: "Time zone",
        kind: {
          type: "text",
          optional: true,
          placeholder: "e.g. Europe/Berlin",
        },
        hint: "Empty for the host's own.",
      },
    ],
  },
  {
    title: "Steps and intervals",
    fields: [
      {
        path: ["learning_steps"],
        label: "Learning steps",
        kind: { type: "steps" },
        hint: "Before a new card graduates, e.g. 1m 10m.",
      },
      {
        path: ["relearning_steps"],
        label: "Relearning steps",
        kind: { type: "steps" },
        hint: "After a review card is forgotten, e.g. 10m.",
      },
      {
        path: ["maximum_interval"],
        label: "Longest interval",
        kind: { type: "duration", unit: "days" },
      },
      {
        path: ["load_balance"],
        label: "Spread reviews evenly over the days around their due date",
        kind: { type: "bool" },
      },
      {
        path: ["disperse_siblings"],
        label: "Keep the cards of a note from falling due on the same day",
        kind: { type: "bool" },
      },
      {
        path: ["auto_optimize"],
        label: "Fit FSRS's parameters to the review history by itself",
        kind: { type: "bool" },
        hint: "The new parameters are offered to apply only if they predict reviews clearly better.",
      },
    ],
  },
  {
    title: "Review order",
    fields: [
      {
        path: ["queue", "review_order"],
        label: "Reviews",
        kind: { type: "select", options: REVIEW_ORDERS },
      },
      {
        path: ["queue", "new_order"],
        label: "New cards",
        kind: { type: "select", options: NEW_ORDERS },
      },
      {
        path: ["queue", "new_review_mix"],
        label: "New cards and reviews",
        kind: {
          type: "select",
          options: [
            { value: "interleaved", label: "Mixed" },
            { value: "new_first", label: "New cards first" },
            { value: "reviews_first", label: "Reviews first" },
          ],
        },
      },
      {
        path: ["queue", "interleave_by"],
        label: "Take turns between",
        kind: {
          type: "select",
          options: [
            { value: "none", label: "Nothing" },
            { value: "top_level_tag", label: "Subjects (top-level tags)" },
            { value: "preset", label: "Presets" },
          ],
        },
      },
      {
        path: ["queue", "learn_ahead_limit"],
        label: "Show learning cards this early",
        kind: { type: "duration", unit: "minutes" },
      },
      {
        path: ["queue", "timebox"],
        label: "Timebox",
        kind: { type: "duration", unit: "minutes", optional: true },
        hint: "Say how a session is going every so often. Empty for never.",
      },
      {
        path: ["queue", "bury_new_siblings"],
        label: "After a new card, bury its note's other new cards",
        kind: {
          type: "select",
          options: [
            { value: false, label: "No" },
            { value: true, label: "Yes" },
            {
              value: "unless_blocking",
              label: "Unless other cards build on them",
            },
          ],
        },
      },
      {
        path: ["queue", "bury_review_siblings"],
        label: "After a review, bury its note's other reviews",
        kind: { type: "bool" },
      },
    ],
  },
  {
    title: "Leeches",
    fields: [
      {
        path: ["leech", "lapses_threshold"],
        label: "A card is a leech once forgotten",
        kind: { type: "number", min: 1 },
        hint: "Times, after it graduated.",
      },
      {
        path: ["leech", "action"],
        label: "Then",
        kind: {
          type: "select",
          options: [
            { value: "tag", label: "Tag it" },
            { value: "suspend", label: "Tag and suspend it" },
            { value: "none", label: "Nothing" },
          ],
        },
      },
      {
        path: ["leech", "tag_name"],
        label: "Tag",
        kind: { type: "text" },
      },
    ],
  },
  {
    title: "Statistics",
    fields: [
      {
        path: ["statistics", "mature_threshold_days"],
        label: "A card is mature from an interval of (days)",
        kind: { type: "number", min: 1 },
      },
    ],
  },
];

/** A value of a field as its input shows it. */
export function displayValue(kind: FieldKind, value: unknown): string {
  switch (kind.type) {
    case "steps":
      return Array.isArray(value) ? formatSteps(value as number[]) : "";
    case "tags":
      return Array.isArray(value) ? (value as string[]).join(", ") : "";
    case "parts":
      return value && typeof value === "object"
        ? Object.entries(value as Record<string, string>)
            .map(([name, query]) => `${name}: ${query}`)
            .join("\n")
        : "";
    case "date":
      return typeof value === "string" ? value : "";
    case "duration": {
      if (typeof value !== "number") return "";
      const size = kind.unit === "days" ? 86_400 : 60;
      return String(Math.round((value / size) * 100) / 100);
    }
    case "limit":
      // `u32::MAX`, the default, is no limit
      return typeof value === "number" && value < 4_294_967_295
        ? String(value)
        : "";
    case "number":
      return typeof value === "number" ? String(value) : "";
    case "text":
      return typeof value === "string" ? value : "";
    case "bool":
      return JSON.stringify(value);
    case "select":
      return value === null || value === undefined ? "" : JSON.stringify(value);
  }
}

/** What an input's text sets: a value, `undefined` to take the default, or an error. */
export type Parsed = { value: unknown } | { remove: true } | { error: string };

export function parseInput(kind: FieldKind, text: string): Parsed {
  const trimmed = text.trim();
  if (trimmed === "" && "optional" in kind && kind.optional)
    return { remove: true };
  switch (kind.type) {
    case "steps": {
      const steps = parseSteps(trimmed);
      return typeof steps === "string" ? { error: steps } : { value: steps };
    }
    case "tags":
      return { value: trimmed.split(/[\s,]+/).filter(Boolean) };
    case "parts": {
      const parts: Record<string, string> = {};
      for (const line of text.split("\n")) {
        if (line.trim() === "") continue;
        const colon = line.indexOf(":");
        if (colon <= 0 || line.slice(colon + 1).trim() === "")
          return { error: `"${line.trim()}" is not "name: search"` };
        parts[line.slice(0, colon).trim()] = line.slice(colon + 1).trim();
      }
      return { value: parts };
    }
    case "date":
      return /^\d{4}-\d{2}-\d{2}$/.test(trimmed)
        ? { value: trimmed }
        : { error: "Not a date, such as 2027-05-01" };
    case "duration": {
      const number = Number(trimmed);
      if (trimmed === "" || !Number.isFinite(number) || number < 0)
        return { error: `Not a number of ${kind.unit}` };
      const size = kind.unit === "days" ? 86_400 : 60;
      return { value: Math.round(number * size) };
    }
    case "limit":
    case "number": {
      if (trimmed === "" && kind.type === "limit") return { remove: true };
      const number = Number(trimmed);
      const fraction = kind.type === "number" && kind.fraction;
      if (
        trimmed === "" ||
        !Number.isFinite(number) ||
        (!fraction && !Number.isInteger(number))
      )
        return { error: fraction ? "Not a number" : "Not a whole number" };
      if (kind.type === "number" && kind.min !== undefined && number < kind.min)
        return { error: `At least ${kind.min}` };
      if (kind.type === "number" && kind.max !== undefined && number > kind.max)
        return { error: `At most ${kind.max}` };
      if (number < 0) return { error: "At least 0" };
      return { value: number };
    }
    case "text":
      return { value: text };
    case "bool":
    case "select":
      return { value: JSON.parse(text) as unknown };
  }
}

/** A preset's settings in the form (see `QueuePreset` in `spares_core::config`). Those left
    empty follow the top-level ones. */
export const PRESET_FIELDS: {
  key: string;
  label: string;
  kind: FieldKind;
  hint?: string;
}[] = [
  {
    key: "tags",
    label: "Notes tagged",
    kind: { type: "tags" },
    hint: "Or under one of these tags, e.g. lang matches lang:german.",
  },
  {
    key: "query",
    label: "Or cards matching",
    kind: { type: "text", optional: true, placeholder: "a search", wide: true },
  },
  {
    key: "new_cards_daily_limit",
    label: "New cards a day",
    kind: { type: "number", min: 0, optional: true },
  },
  {
    key: "reviews_daily_limit",
    label: "Reviews a day",
    kind: { type: "number", min: 0, optional: true },
  },
  {
    key: "new_order",
    label: "New cards",
    kind: { type: "select", options: NEW_ORDERS, optional: "As above" },
  },
  {
    key: "review_order",
    label: "Reviews",
    kind: { type: "select", options: REVIEW_ORDERS, optional: "As above" },
  },
  {
    key: "learning_steps",
    label: "Learning steps",
    kind: { type: "steps", optional: true },
  },
  {
    key: "relearning_steps",
    label: "Relearning steps",
    kind: { type: "steps", optional: true },
  },
];

/** A goal's settings in the form (see `Goal` in `spares_core::config`). Its milestones are in
    the TOML editor. */
export const GOAL_FIELDS: {
  key: string;
  label: string;
  kind: FieldKind;
  hint?: string;
}[] = [
  { key: "date", label: "Date", kind: { type: "date" } },
  {
    key: "query",
    label: "Cards",
    kind: { type: "text", placeholder: "a search", wide: true },
  },
  {
    key: "learned_by",
    label: "Learned by",
    kind: { type: "date", optional: true },
    hint: "When every new card should be learned, for the pace it reports.",
  },
  {
    key: "retention",
    label: "Recall on the day",
    kind: { type: "number", min: 0, max: 1, fraction: true, optional: true },
    hint: "e.g. 0.95: rating one of its cards keeps it due before the date if it would be recalled less.",
  },
  {
    key: "parts",
    label: "Parts",
    kind: { type: "parts" },
    hint: "One a line, as name: search, each reported on its own.",
  },
];

/** A change to one setting not saved yet: its new value, or `undefined` for its default. */
export interface Draft {
  path: SettingPath;
  value: unknown;
  remove: boolean;
}

export function pathKey(path: SettingPath): string {
  return path.join("\u0000");
}

/** The changes as `PATCH /api/settings` takes them. A removed table goes first, and none of the
    changes inside it are sent. */
export function draftValues(drafts: Draft[]): SettingValue[] {
  const removed = drafts
    .filter((draft) => draft.remove)
    .map((draft) => draft.path);
  const inside = (path: SettingPath) =>
    removed.some(
      (table) =>
        table.length < path.length && table.every((key, i) => path[i] === key),
    );
  return drafts
    .filter((draft) => !inside(draft.path))
    .sort((a, b) => Number(b.remove) - Number(a.remove))
    .map((draft) =>
      draft.remove
        ? { key: draft.path }
        : { key: draft.path, value: draft.value },
    );
}
