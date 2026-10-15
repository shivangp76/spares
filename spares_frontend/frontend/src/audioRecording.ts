// Recording audio clips in the browser. Browsers record in different formats, and only some of
// them are audio files cards accept (see `isAudioPath`), so a recording in another format, such as
// Chrome's WebM, is converted to WAV before it is uploaded.

/** AAC in MP4, which every client, `afplay` included, can play. Safari records it. */
const M4A_TYPE = 'audio/mp4;codecs=mp4a.40.2';

/** The type to ask `MediaRecorder` for, or `undefined` for the browser's own. */
export function recordingMimeType(): string | undefined {
  return typeof MediaRecorder !== 'undefined' && MediaRecorder.isTypeSupported(M4A_TYPE) ? M4A_TYPE : undefined;
}

/** Whether the browser can record audio here. Microphones are only available on secure pages
    (HTTPS or localhost). */
export function canRecordAudio(): boolean {
  return window.isSecureContext && !!navigator.mediaDevices?.getUserMedia && typeof MediaRecorder !== 'undefined';
}

/** `samples` as a 16-bit mono WAV file. */
export function encodeWav(samples: Float32Array, sampleRate: number): Blob {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  const writeString = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i));
  };
  writeString(0, 'RIFF');
  view.setUint32(4, 36 + samples.length * 2, true);
  writeString(8, 'WAVE');
  writeString(12, 'fmt ');
  view.setUint32(16, 16, true); // Size of the format chunk
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // Mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // Bytes per second
  view.setUint16(32, 2, true); // Bytes per sample
  view.setUint16(34, 16, true); // Bits per sample
  writeString(36, 'data');
  view.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return new Blob([buffer], { type: 'audio/wav' });
}

/** The channels of `audio` averaged into one. */
function downmix(audio: AudioBuffer): Float32Array {
  const mono = new Float32Array(audio.length);
  for (let c = 0; c < audio.numberOfChannels; c++) {
    const channel = audio.getChannelData(c);
    for (let i = 0; i < audio.length; i++) mono[i] += channel[i] / audio.numberOfChannels;
  }
  return mono;
}

/** A recording as a file to upload, named with an extension cards accept. */
export async function recordingFile(recording: Blob): Promise<File> {
  const name = `recording-${new Date().toISOString().replace(/[:.]/g, '-')}`;
  if (recording.type.startsWith('audio/mp4')) return new File([recording], `${name}.m4a`, { type: 'audio/mp4' });
  const context = new AudioContext();
  try {
    const audio = await context.decodeAudioData(await recording.arrayBuffer());
    return new File([encodeWav(downmix(audio), audio.sampleRate)], `${name}.wav`, { type: 'audio/wav' });
  } finally {
    void context.close();
  }
}
