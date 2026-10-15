import { expect, test } from 'bun:test';
import { imageUploadName } from './utils';

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
