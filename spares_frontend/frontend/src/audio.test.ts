import { expect, test } from 'bun:test';
import { audioLinkPath, audioMimeType, isAudioPath } from './audio';

test('isAudioPath', () => {
  for (const path of ['/a/b.mp3', 'c.M4A', 'd.ogg', 'e.opus', 'f.wav', 'g.flac']) expect(isAudioPath(path)).toBe(true);
  for (const path of ['/a/b.png', 'li', 'mp3', '/a/.mp3', 'https://example.com']) expect(isAudioPath(path)).toBe(false);
});

test('audioLinkPath', () => {
  expect(audioLinkPath('/m/with%20space.mp3')).toBe('/m/with space.mp3');
  expect(audioLinkPath('/m/a.png')).toBe(null);
  expect(audioLinkPath('https://example.com/a.mp3')).toBe(null);
  expect(audioLinkPath(null)).toBe(null);
  expect(audioMimeType('/m/a.MP3')).toBe('audio/mpeg');
});
