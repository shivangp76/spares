import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchRenderAsset } from '../api/client';
import { audioMimeType } from '../audio';
import { errorMessage } from '../utils';

// Object URLs of fetched clips, so replaying does not fetch again. The oldest are revoked.
const MAX_CACHED_CLIPS = 32;
const clipUrls = new Map<string, Promise<string>>();

function clipUrl(path: string): Promise<string> {
  let url = clipUrls.get(path);
  if (!url) {
    url = fetchRenderAsset(path).then(data => {
      if (data === null) throw new Error(`The server has no audio clip ${path}. Render the note locally, then sync.`);
      return URL.createObjectURL(new Blob([data as BlobPart], { type: audioMimeType(path) }));
    });
    // A failed fetch is retried next time
    url.catch(() => clipUrls.delete(path));
    clipUrls.set(path, url);
    while (clipUrls.size > MAX_CACHED_CLIPS) {
      const [oldest, oldestUrl] = clipUrls.entries().next().value!;
      clipUrls.delete(oldest);
      oldestUrl.then(URL.revokeObjectURL, () => {});
    }
  }
  return url;
}

export interface AudioPlayer {
  /** Plays `paths` one after another, stopping whatever was playing. */
  play: (paths: string[]) => void;
  stop: () => void;
  /** The clip playing, if any. */
  playing: string | null;
  /** Clips the browser would not autoplay before the page was interacted with. */
  blocked: string[] | null;
  error: string | null;
}

/** Plays a card's audio clips, one at a time. */
export function useAudioPlayer(): AudioPlayer {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  // Bumped by every `play` and `stop`, so a superseded queue stops
  const generation = useRef(0);
  const [playing, setPlaying] = useState<string | null>(null);
  const [blocked, setBlocked] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const stop = useCallback(() => {
    generation.current += 1;
    audioRef.current?.pause();
    setPlaying(null);
    setBlocked(null);
  }, []);

  const play = useCallback((paths: string[]) => {
    stop();
    setError(null);
    const current = generation.current;
    const audio = audioRef.current ?? (audioRef.current = new Audio());
    const playFrom = async (index: number) => {
      if (generation.current !== current) return;
      if (index >= paths.length) { setPlaying(null); return; }
      const path = paths[index];
      try {
        const url = await clipUrl(path);
        if (generation.current !== current) return;
        audio.src = url;
        audio.onended = () => playFrom(index + 1);
        setPlaying(path);
        await audio.play();
      } catch (e) {
        if (generation.current !== current) return;
        setPlaying(null);
        if (e instanceof DOMException && e.name === 'NotAllowedError') {
          setBlocked(paths.slice(index));
          return;
        }
        setError(errorMessage(e));
        playFrom(index + 1);
      }
    };
    playFrom(0);
  }, [stop]);

  useEffect(() => () => {
    generation.current += 1;
    audioRef.current?.pause();
  }, []);

  return { play, stop, playing, blocked, error };
}
