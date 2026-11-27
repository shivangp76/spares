import { expect, test } from 'bun:test';
import { isUnspaced, lookupStart, vocabularyNote } from './text';

test('Han and kana start at the character tapped, other words at their start', () => {
  expect(isUnspaced('漢')).toBe(true);
  expect(isUnspaced('か')).toBe(true);
  expect(isUnspaced('カ')).toBe(true);
  expect(isUnspaced('a')).toBe(false);
  expect(isUnspaced('한')).toBe(false);
  expect(lookupStart('私は日本語', 3)).toBe(3);
  expect(lookupStart('I was running fast', 9)).toBe(6);
  expect(lookupStart("don't stop", 3)).toBe(0);
  expect(lookupStart('한국어 공부', 5)).toBe(4);
  // A word right after Han text starts after it
  expect(lookupStart('漢字abc', 3)).toBe(2);
});

test('Add card writes the word with its reading in a cloze', () => {
  expect(vocabularyNote({ expression: '食べる', reading: 'たべる', glosses: ['to eat', 'to live on'] })).toBe(
    '{{食べる[たべる]}}: to eat; to live on',
  );
  expect(vocabularyNote({ expression: 'run', reading: '', glosses: ['• to move\n• to flow'] })).toBe(
    '{{run}}: • to move; • to flow',
  );
  expect(vocabularyNote({ expression: 'かな', reading: 'かな', glosses: [] })).toBe('{{かな}}');
});
