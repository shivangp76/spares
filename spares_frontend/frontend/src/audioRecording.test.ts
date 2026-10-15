import { expect, test } from 'bun:test';
import { encodeWav } from './audioRecording';

test('encodeWav writes a 16-bit mono WAV', async () => {
  const wav = encodeWav(new Float32Array([0, 1, -1, 2]), 8000);
  expect(wav.type).toBe('audio/wav');
  const view = new DataView(await wav.arrayBuffer());
  const text = (offset: number) => String.fromCharCode(...new Uint8Array(view.buffer, offset, 4));
  expect([text(0), text(8), text(12), text(36)]).toEqual(['RIFF', 'WAVE', 'fmt ', 'data']);
  expect(view.byteLength).toBe(44 + 4 * 2);
  expect(view.getUint32(4, true)).toBe(view.byteLength - 8);
  expect(view.getUint16(22, true)).toBe(1);
  expect(view.getUint32(24, true)).toBe(8000);
  expect(view.getUint32(40, true)).toBe(8);
  // Samples beyond [-1, 1] are clipped
  expect([0, 1, 2, 3].map(i => view.getInt16(44 + i * 2, true))).toEqual([0, 32767, -32768, 32767]);
});
