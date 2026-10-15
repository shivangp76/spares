// Audio clips: links to audio files in a card (see `spares_core/src/parsers/audio.rs`). Clients
// play them outside the rendered card, fetched from the server's render assets.

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
