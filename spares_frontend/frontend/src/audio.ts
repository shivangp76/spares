// Audio clips: links to audio files in a card (see `spares_core/src/parsers/audio.rs`). Clients
// play them outside the rendered card, fetched from the server's render assets. Cards can also
// have text to read aloud (see `spares_core/src/parsers/tts.rs`), played along with the clips.

import type { AudioItem, CardAudio, SpeechItem } from './types/spares';

const MIME_TYPES: Record<string, string> = {
  mp3: 'audio/mpeg',
  m4a: 'audio/mp4',
  aac: 'audio/aac',
  ogg: 'audio/ogg',
  oga: 'audio/ogg',
  opus: 'audio/ogg',
  wav: 'audio/wav',
  flac: 'audio/flac',
};

function extension(path: string): string {
  const name = path.split(/[?#]/, 1)[0].split('/').pop() ?? '';
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** Whether `path` names an audio file, judged by its extension like the server does. */
export function isAudioPath(path: string): boolean {
  return extension(path) in MIME_TYPES;
}

/** The media type of the audio file `path`, which the server does not send. */
export function audioMimeType(path: string): string {
  return MIME_TYPES[extension(path)] ?? 'application/octet-stream';
}

/** The absolute path an audio link in a rendered card points at, or `null` if it is not one. */
export function audioLinkPath(href: string | null): string | null {
  if (!href || !href.startsWith('/') || !isAudioPath(href)) return null;
  try {
    // pandoc escapes characters such as spaces
    return decodeURI(href);
  } catch {
    return href;
  }
}

/** The file name of a clip, for its button. */
export function clipName(path: string): string {
  return path.split('/').pop() ?? path;
}

/** Whether `item` is text to read aloud, not a clip. */
export function isSpeech(item: AudioItem): item is SpeechItem {
  return typeof item !== 'string';
}

/** The audio clips among `items`, e.g. to download them. */
export function clipPaths(items: AudioItem[]): string[] {
  return items.filter((item): item is string => !isSpeech(item));
}

/** Tells items apart, e.g. to show which is playing. */
export function audioItemKey(item: AudioItem): string {
  return isSpeech(item) ? `speech:${item.lang ?? ''}:${item.text}` : item;
}

/** What an item's button says: the clip's file name, or the start of the text. */
export function audioItemLabel(item: AudioItem): string {
  if (!isSpeech(item)) return clipName(item);
  return item.text.length > 30 ? `${item.text.slice(0, 29)}…` : item.text;
}

/** `audio` without its text to read aloud, or `undefined` if only that is left. */
export function withoutSpeech(audio: CardAudio | undefined): CardAudio | undefined {
  if (!audio) return undefined;
  const front = audio.front.filter(item => !isSpeech(item));
  const back = audio.back.filter(item => !isSpeech(item));
  return front.length || back.length ? { front, back } : undefined;
}

/** `lang` as browsers write languages: `fr_FR` becomes `fr-FR`. */
export function speechLang(lang: string): string {
  return lang.replace(/_/g, '-');
}

/** The voice of `voices` to read `item` with: the one it names, if the browser has it, or else one
    for its language, preferring the browser's default. `null` leaves the choice to the browser. */
export function pickVoice<V extends Pick<SpeechSynthesisVoice, 'name' | 'lang' | 'default'>>(
  voices: readonly V[], item: SpeechItem,
): V | null {
  if (item.voice) {
    const name = item.voice.toLowerCase();
    const named = voices.find(v => v.name.toLowerCase() === name) ?? voices.find(v => v.name.toLowerCase().includes(name));
    if (named) return named;
  }
  if (!item.lang) return null;
  const lang = speechLang(item.lang).toLowerCase();
  const primary = lang.split('-')[0];
  const byDefault = (candidates: V[]) => candidates.find(v => v.default) ?? candidates[0];
  return byDefault(voices.filter(v => speechLang(v.lang).toLowerCase() === lang))
    ?? byDefault(voices.filter(v => speechLang(v.lang).toLowerCase().split('-')[0] === primary))
    ?? null;
}
