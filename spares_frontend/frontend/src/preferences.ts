import { useSyncExternalStore } from 'react';

const CHANGE_EVENT = 'spares-preferences-change';

/** A setting kept in this browser, falling back to `defaultValue` when unset or without storage. */
function booleanPreference(key: string, defaultValue: () => boolean) {
  function get(): boolean {
    try {
      const stored = localStorage.getItem(key);
      return stored === null ? defaultValue() : stored === 'true';
    } catch {
      // Without storage the preference can't be remembered, so the default is used
      return defaultValue();
    }
  }

  function set(value: boolean): void {
    try {
      localStorage.setItem(key, String(value));
    } catch {
      // Without storage the preference can't be changed, so the default stays
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }

  function use(): boolean {
    return useSyncExternalStore(subscribe, get);
  }

  return { get, set, use };
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, onChange);
  return () => window.removeEventListener(CHANGE_EVENT, onChange);
}

/** Whether the review page shows the running recall and rate timers. Shown unless turned off. */
const showReviewTimer = booleanPreference('spares_show_review_timer', () => true);
export const setShowReviewTimer = showReviewTimer.set;
export const useShowReviewTimer = showReviewTimer.use;

/** Whether note editors use Vim keybindings. Off by default on touchscreens, where a phone
    keyboard can't leave Vim's normal mode to type. */
const vimKeybindings = booleanPreference('spares_vim_keybindings', () => !window.matchMedia('(pointer: coarse)').matches);
export const setVimKeybindings = vimKeybindings.set;
export const useVimKeybindings = vimKeybindings.use;
