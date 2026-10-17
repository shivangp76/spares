import { expect, test } from 'bun:test';
import { draggedImageUrl, highlightPreview, imageUploadName, insertInline, withPage } from './utils';

test('imageUploadName keeps a supported extension', () => {
  expect(imageUploadName({ name: 'brain.PNG', type: 'image/png' })).toBe('brain.PNG');
  expect(imageUploadName({ name: 'diagram.svg', type: '' })).toBe('diagram.svg');
});

test('imageUploadName takes the extension from the type otherwise', () => {
  expect(imageUploadName({ name: '', type: 'image/png' })).toBe('image.png');
  expect(imageUploadName({ name: 'Screenshot', type: 'image/jpeg' })).toBe('Screenshot.jpg');
  expect(imageUploadName({ name: 'photo.jfif', type: 'image/jpeg' })).toBe('photo.jpg');
});

test('imageUploadName rejects formats cards cannot show', () => {
  expect(imageUploadName({ name: 'photo.heic', type: 'image/heic' })).toBeNull();
  expect(imageUploadName({ name: 'notes.txt', type: 'text/plain' })).toBeNull();
});

test('draggedImageUrl finds the dragged image', () => {
  expect(draggedImageUrl('<img alt="770 × 533" src="https://a.org/cell.png?w=1&amp;h=2">')).toBe('https://a.org/cell.png?w=1&h=2');
  expect(draggedImageUrl("<a href='/x'><img class=x src='http://a.org/b.jpg'></a>")).toBe('http://a.org/b.jpg');
  expect(draggedImageUrl('<img src=data:image/png;base64,AA==>')).toBe('data:image/png;base64,AA==');
  expect(draggedImageUrl('<img data-src="x" src="https://a.org/c.gif">')).toBe('https://a.org/c.gif');
});

test('draggedImageUrl ignores links and unusable images', () => {
  expect(draggedImageUrl('<a href="https://a.org/cell.png">cell</a>')).toBeNull();
  expect(draggedImageUrl('<img src="/relative.png">')).toBeNull();
  expect(draggedImageUrl('<img src="blob:https://a.org/1234">')).toBeNull();
  expect(draggedImageUrl('')).toBeNull();
});

test('insertInline appends after a space without an editor', () => {
  expect(insertInline(undefined, '', '[listen](/a.wav)')).toBe('[listen](/a.wav)');
  expect(insertInline(undefined, 'Say', '[listen](/a.wav)')).toBe('Say [listen](/a.wav)');
  expect(insertInline(undefined, 'Say\n', '[listen](/a.wav)')).toBe('Say\n[listen](/a.wav)');
});

test('highlightPreview marks the matches in the preview', () => {
  const show = (text: string, matches: [number, number][], max: number) =>
    highlightPreview(text, matches, max).map(p => (p.mark ? `[${p.text}]` : p.text)).join('');
  expect(show('a hot dog', [[6, 9]], 100)).toBe('a hot [dog]');
  expect(show('dog and dog', [[0, 3], [8, 11]], 9)).toBe('[dog] and [d]…');
  expect(show('short', [], 3)).toBe('sho…');
  // A match past the end of the preview is shown with some text before it
  const long = `${'x'.repeat(100)} the dog sat`;
  expect(show(long, [[105, 108]], 50)).toBe(`…${'x'.repeat(25)} the [dog] sat`);
  // Not between the halves of an emoji
  expect(show('😀😀', [], 3)).toBe('😀…');
});

test('withPage keeps the search', () => {
  expect(withPage(new URLSearchParams('q=dog'), 3).toString()).toBe('q=dog&page=3');
  expect(withPage(new URLSearchParams('q=dog&page=3'), 1).toString()).toBe('q=dog');
});
