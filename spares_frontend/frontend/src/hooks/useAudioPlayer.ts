import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchRenderAsset } from '../api/client';
import { audioItemKey, audioMimeType, isSpeech, pickVoice, speechLang } from '../audio';
import type { AudioItem, SpeechItem } from '../types/spares';
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

/** Raised when the browser would not play before the page was interacted with. */
class BlockedError extends Error {}

/** The browser's voices. Some browsers list them only after a while, so this waits a little for
    them, and gives up if there are none. */
function speechVoices(): Promise<SpeechSynthesisVoice[]> {
  const voices = speechSynthesis.getVoices();
  if (voices.length > 0) return Promise.resolve(voices);
  return new Promise(resolve => {
    const done = () => {
      speechSynthesis.removeEventListener('voiceschanged', done);
      clearTimeout(timeout);
      resolve(speechSynthesis.getVoices());
    };
    const timeout = setTimeout(done, 1000);
    speechSynthesis.addEventListener('voiceschanged', done);
  });
}

/** Reads `item` aloud, resolving when it has been read or was stopped. */
async function speak(item: SpeechItem): Promise<void> {
  if (typeof speechSynthesis === 'undefined') throw new Error('This browser cannot read text aloud.');
  const utterance = new SpeechSynthesisUtterance(item.text);
  if (item.lang) utterance.lang = speechLang(item.lang);
  if (item.speed) utterance.rate = item.speed;
  const voice = pickVoice(await speechVoices(), item);
  if (voice) {
    utterance.voice = voice;
    if (!item.lang) utterance.lang = voice.lang;
  }
  await new Promise<void>((resolve, reject) => {
    utterance.onend = () => resolve();
    utterance.onerror = e => {
      if (e.error === 'interrupted' || e.error === 'canceled') resolve();
      else if (e.error === 'not-allowed') reject(new BlockedError());
      else reject(new Error(`Could not read "${item.text}" aloud: ${e.error}`));
    };
    speechSynthesis.speak(utterance);
  });
}

export interface AudioPlayer {
  /** Plays `items` one after another, stopping whatever was playing. */
  play: (items: AudioItem[]) => void;
  stop: () => void;
  /** The key (see `audioItemKey`) of the item playing, if any. */
  playing: string | null;
  /** Items the browser would not autoplay before the page was interacted with. */
  blocked: AudioItem[] | null;
  error: string | null;
}

/** Plays a card's audio clips, and reads its text aloud, one at a time. */
export function useAudioPlayer(): AudioPlayer {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  // Bumped by every `play` and `stop`, so a superseded queue stops
  const generation = useRef(0);
  const [playing, setPlaying] = useState<string | null>(null);
  const [blocked, setBlocked] = useState<AudioItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const stop = useCallback(() => {
    generation.current += 1;
    audioRef.current?.pause();
    if (typeof speechSynthesis !== 'undefined') speechSynthesis.cancel();
    setPlaying(null);
    setBlocked(null);
  }, []);

  const play = useCallback((items: AudioItem[]) => {
    stop();
    setError(null);
    const current = generation.current;
    const audio = audioRef.current ?? (audioRef.current = new Audio());
    const playClip = async (path: string) => {
      const url = await clipUrl(path);
      if (generation.current !== current) return;
      // Never settled if the clip is stopped, which leaves the superseded queue waiting
      const ended = new Promise<void>((resolve, reject) => {
        audio.onended = () => resolve();
        audio.onerror = () => reject(new Error(`Could not play the audio clip ${path}.`));
      });
      audio.src = url;
      try {
        await audio.play();
      } catch (e) {
        throw e instanceof DOMException && e.name === 'NotAllowedError' ? new BlockedError() : e;
      }
      await ended;
    };
    (async () => {
      for (const [index, item] of items.entries()) {
        if (generation.current !== current) return;
        setPlaying(audioItemKey(item));
        try {
          await (isSpeech(item) ? speak(item) : playClip(item));
        } catch (e) {
          if (generation.current !== current) return;
          if (e instanceof BlockedError) {
            setPlaying(null);
            setBlocked(items.slice(index));
            return;
          }
          setError(errorMessage(e));
        }
      }
      if (generation.current === current) setPlaying(null);
    })();
  }, [stop]);

  useEffect(() => () => {
    generation.current += 1;
    audioRef.current?.pause();
    if (typeof speechSynthesis !== 'undefined') speechSynthesis.cancel();
  }, []);

  return { play, stop, playing, blocked, error };
}
