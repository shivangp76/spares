import { expect, test } from 'bun:test';
import { applyCompletion, needsQuotes, searchCompletions } from './searchCompletion';
import type { SearchField } from './types/spares';

const field = (name: string, sortable = true): SearchField => ({ name, kind: '', example: '', about: '', sortable });
const fields = [field('tag'), field('c.due'), field('c.state'), field('c.stability'), field('created_at'), field('c.cloze', false)];
const tags = ['math', 'math:algebra', 'physics', 'Mathematics'];

/** The labels suggested with the caret at `|` in `query`. */
function labels(query: string): string[] {
  const caret = query.indexOf('|');
  return searchCompletions(query.replace('|', ''), caret, fields, tags)?.items.map(i => i.label) ?? [];
}

test('field names complete as they are typed', () => {
  expect(labels('ta|')).toEqual(['tag']);
  expect(labels('dog c.st|')).toEqual(['c.state', 'c.stability']);
  // The `c.` can be left out
  expect(labels('du|')).toEqual(['c.due']);
  expect(labels('-cr|')).toEqual(['created_at']);
  expect(labels('(ta|')).toEqual(['tag']);
});

test('nothing completes a whole field, a keyword or quoted text', () => {
  expect(labels('tag|')).toEqual([]);
  expect(labels('dog or|')).toEqual([]);
  expect(labels('"ta|')).toEqual([]);
  expect(labels('|')).toEqual([]);
});

test('tags complete after tag=', () => {
  expect(labels('tag=ma|')).toEqual(['math', 'math:algebra', 'Mathematics']);
  expect(labels('-tag=ph|')).toEqual(['physics']);
  expect(labels('tag~alg|')).toEqual(['math:algebra']);
  expect(labels('tag="math:a|')).toEqual(['math:algebra']);
});

test('card states and sortable fields complete', () => {
  expect(labels('c.state=re|')).toEqual(['review', 'relearning']);
  expect(labels('sort_by_asc=c.|')).toEqual(['c.due', 'c.state', 'c.stability']);
  expect(labels('sort_by_desc=c.due,cr|')).toEqual(['created_at']);
});

test('a completion replaces the term the caret is in', () => {
  const apply = (query: string, label: string) => {
    const caret = query.indexOf('|');
    const plain = query.replace('|', '');
    const completions = searchCompletions(plain, caret, fields, tags)!;
    const { query: result, caret: after } = applyCompletion(plain, completions, completions.items.find(i => i.label === label)!);
    return result.slice(0, after) + '|' + result.slice(after);
  };
  expect(apply('dog ta| cat', 'tag')).toBe('dog tag| cat');
  expect(apply('st|ate', 'c.state')).toBe('c.state|');
  expect(apply('tag=math:| dog', 'math:algebra')).toBe('tag="math:algebra"| dog');
  expect(apply('tag="math:a|"', 'math:algebra')).toBe('tag="math:algebra"|');
  expect(apply('sort_by_asc=c.d|,created_at', 'c.due')).toBe('sort_by_asc=c.due|,created_at');
});

test('values are quoted when they have to be', () => {
  expect(needsQuotes('math')).toBe(false);
  expect(needsQuotes('how-a-car-works')).toBe(false);
  expect(needsQuotes('math:algebra')).toBe(true);
  expect(needsQuotes('-math')).toBe(true);
  expect(needsQuotes('a b')).toBe(true);
});
