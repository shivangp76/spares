import { expect, test } from 'bun:test';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { PAGE_HELP, headingAnchor } from './pages';

const ROOT = join(import.meta.dir, '../../../..');

/** The anchors of the headings of a file of the manual, leaving out code blocks. */
function anchors(file: string): string[] {
  const text = readFileSync(join(ROOT, 'docs/src', file), 'utf8');
  const withoutCode = text.replace(/^```[\s\S]*?^```/gm, '');
  return [...withoutCode.matchAll(/^#+\s+(.*)$/gm)].map(m => headingAnchor(m[1]));
}

/** The fields a query can use, from the table the server lists them from. */
const FIELDS = [...readFileSync(join(ROOT, 'spares_core/src/search/evaluator.rs'), 'utf8')
  .matchAll(/^\s*doc\("([^"]+)",/gm)].map(m => m[1]);

/** The code in a text, as written between backticks. */
const code = (text: string) => [...text.matchAll(/`([^`]*)`/g)].map(m => m[1]);

test('the manual has every linked section', () => {
  for (const [path, help] of Object.entries(PAGE_HELP)) {
    for (const link of help.docs) {
      expect(existsSync(join(ROOT, 'docs/src', link.file)), `${path}: ${link.file}`).toBe(true);
      if (link.heading) expect(anchors(link.file), `${path}: ${link.heading}`).toContain(headingAnchor(link.heading));
    }
  }
});

test('the queries in the help use fields that exist', () => {
  expect(FIELDS).toContain('c.due');
  for (const [path, help] of Object.entries(PAGE_HELP)) {
    for (const span of help.about.flatMap(code)) {
      const field = /^-?([\w.]+?)(!=|<=|>=|=|~|<|>)/.exec(span)?.[1];
      if (field) expect(FIELDS, `${path}: ${span}`).toContain(field);
    }
  }
});

test('the modules the help names exist', () => {
  for (const [path, help] of Object.entries(PAGE_HELP)) {
    for (const span of help.about.flatMap(code).filter(s => s.includes('::'))) {
      const [crate, ...modules] = span.split('::');
      const dir = join(ROOT, crate, 'src', ...modules);
      expect(existsSync(dir) || existsSync(`${dir}.rs`), `${path}: ${span}`).toBe(true);
    }
  }
});

test('the fields whose descriptions explain values exist', () => {
  const sources = readdirSync(join(import.meta.dir, '..'), { recursive: true, encoding: 'utf8' })
    .filter(file => file.endsWith('.tsx'));
  const named = sources.flatMap(file =>
    [...readFileSync(join(import.meta.dir, '..', file), 'utf8').matchAll(/\babout\('([^']+)'\)/g)].map(m => m[1]));
  expect(named).toContain('c.stability');
  for (const name of named) expect(FIELDS).toContain(name);
});

test('headingAnchor', () => {
  expect(headingAnchor('Creating a parser (optional)')).toBe('creating-a-parser-optional');
  expect(headingAnchor('Note vs Card')).toBe('note-vs-card');
});
