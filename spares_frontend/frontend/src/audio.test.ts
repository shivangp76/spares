import { expect, test } from 'bun:test';
import { audioItemLabel, audioLinkPath, audioMimeType, clipPaths, isAudioPath, pickVoice, withoutSpeech } from './audio';

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

test('speech items', () => {
  const speech = { text: 'bonjour', lang: 'fr_FR' };
  expect(clipPaths(['/m/a.mp3', speech])).toEqual(['/m/a.mp3']);
  expect(audioItemLabel(speech)).toBe('bonjour');
  expect(audioItemLabel('/m/a.mp3')).toBe('a.mp3');
  expect(audioItemLabel({ text: 'x'.repeat(40) })).toBe(`${'x'.repeat(29)}…`);
  expect(withoutSpeech({ front: ['/m/a.mp3', speech], back: [speech] })).toEqual({ front: ['/m/a.mp3'], back: [] });
  expect(withoutSpeech({ front: [speech], back: [] })).toBe(undefined);
});

test('pickVoice', () => {
  const voices = [
    { name: 'Samantha', lang: 'en-US', default: true },
    { name: 'Thomas', lang: 'fr-FR', default: false },
    { name: 'Amélie', lang: 'fr-CA', default: false },
    { name: 'Google français', lang: 'fr-FR', default: false },
  ];
  // By name, also in part
  expect(pickVoice(voices, { text: 'x', voice: 'amélie' })?.name).toBe('Amélie');
  expect(pickVoice(voices, { text: 'x', voice: 'Google' })?.name).toBe('Google français');
  // A voice the browser lacks falls back to the language
  expect(pickVoice(voices, { text: 'x', lang: 'fr_FR', voice: 'Paul' })?.name).toBe('Thomas');
  // Another region of the language
  expect(pickVoice(voices, { text: 'x', lang: 'fr-BE' })?.name).toBe('Thomas');
  expect(pickVoice(voices, { text: 'x', lang: 'de' })).toBe(null);
  expect(pickVoice(voices, { text: 'x' })).toBe(null);
});
