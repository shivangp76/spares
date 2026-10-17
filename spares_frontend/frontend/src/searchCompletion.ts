import type { SearchField } from './types/spares';

/** A suggestion for the search box: `insert` replaces the text from `from` to `to`. */
export interface Completion {
  label: string;
  insert: string;
  detail?: string;
}

export interface Completions {
  from: number;
  to: number;
  items: Completion[];
}

const KEYWORDS = ['and', 'or', 'not'];
const OPERATOR = /^([\p{L}\p{N}_.[\]]+?)(!=|<=|>=|=|~|<|>)(.*)$/su;

/** The names a field can hold, like the card states of `c.state`, which is also written `card.state`. */
function valuesOf(field: string, fields: SearchField[]): string[] {
  const name = field.replace(/^card\./, 'c.');
  return fields.find(f => f.name === name)?.values ?? [];
}

/** Whether `c` ends a term, outside quotes. */
const endsTerm = (c: string) => /[\s()"]/.test(c);

/** Whether a tag has to be quoted to be written as a value, as `tag="spanish:verbs"` does. */
export function needsQuotes(value: string): boolean {
  return !/^[\p{L}\p{N}._+][\p{L}\p{N}._+-]*$/u.test(value);
}

/** The value as written in a query, quoted if it has to be. */
function written(value: string): string {
  return needsQuotes(value) ? `"${value.replace(/"/g, '\\"')}"` : value;
}

/**
 * The suggestions for the term the caret is at in `query`: field names while a field is typed,
 * and after `tag=`, a field of a few values like `c.state=`, or a sort, the tags, values or sortable
 * fields. Null when there are none.
 */
export function searchCompletions(
  query: string,
  caret: number,
  fields: SearchField[],
  tags: string[],
  max = 8,
): Completions | null {
  const before = query.slice(0, caret);
  // Inside quotes when an odd number of unescaped quotes come before the caret
  const quotes = [...before.matchAll(/(?<!\\)"/g)].map(m => m.index);
  const openQuote = quotes.length % 2 === 1 ? quotes[quotes.length - 1] : null;

  let start = openQuote ?? caret;
  while (start > 0 && !endsTerm(query[start - 1])) start--;
  if (query[start] === '-') start++;
  let term = query.slice(start, openQuote ?? caret);
  let end = caret;
  while (end < query.length && !endsTerm(query[end])) end++;
  if (openQuote !== null && query[end] === '"') end++;

  const match = OPERATOR.exec(term);
  if (!match) {
    // A field being typed, which can't be in quotes
    if (openQuote !== null || term.length === 0 || KEYWORDS.includes(term.toLowerCase())) return null;
    term = term.toLowerCase();
    const items = fields
      .filter(f => f.name !== term && (f.name.startsWith(term) || (f.name.startsWith('c.') && f.name.slice(2).startsWith(term))))
      .slice(0, max)
      .map(f => ({ label: f.name, insert: f.name, detail: f.about }));
    return items.length > 0 ? { from: start, to: end, items } : null;
  }

  const [, field, op, rest] = match;
  const valueStart = start + field.length + op.length;
  const typed = openQuote !== null ? before.slice(openQuote + 1) : rest;
  const startsWith = (candidate: string, prefix: string) => candidate.toLowerCase().startsWith(prefix.toLowerCase());
  let from = valueStart;
  let candidates: Completion[] = [];
  if (field === 'tag' && ['=', '!=', '~'].includes(op)) {
    const contains = (tag: string) => op === '~' ? tag.toLowerCase().includes(typed.toLowerCase()) : startsWith(tag, typed);
    candidates = tags.filter(t => t !== typed && contains(t)).map(t => ({ label: t, insert: written(t) }));
  } else if (valuesOf(field, fields).length > 0 && ['=', '!='].includes(op)) {
    candidates = valuesOf(field, fields).filter(s => s !== typed && startsWith(s, typed)).map(s => ({ label: s, insert: s }));
  } else if ((field === 'sort_by_asc' || field === 'sort_by_desc') && op === '=' && openQuote === null) {
    // A sort lists fields separated by `,`, and the last one is being typed
    const last = typed.slice(typed.lastIndexOf(',') + 1);
    from = caret - last.length;
    end = caret;
    while (end < query.length && !endsTerm(query[end]) && query[end] !== ',') end++;
    candidates = fields
      .filter(f => f.sortable && f.name !== last && startsWith(f.name, last))
      .map(f => ({ label: f.name, insert: f.name, detail: f.about }));
  }
  const items = candidates.slice(0, max);
  return items.length > 0 ? { from, to: end, items } : null;
}

/** `query` with `completion` applied, and where the caret goes after it. */
export function applyCompletion(query: string, completions: Completions, completion: Completion): { query: string; caret: number } {
  return {
    query: query.slice(0, completions.from) + completion.insert + query.slice(completions.to),
    caret: completions.from + completion.insert.length,
  };
}
