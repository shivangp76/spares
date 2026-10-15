import { expect, test } from 'bun:test';
import { draggedImageUrl, imageUploadName } from './utils';

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
