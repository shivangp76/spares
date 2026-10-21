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

/** A setting kept in this browser that is one of `values`, falling back to `defaultValue`. */
function choicePreference<T extends string>(key: string, values: readonly T[], defaultValue: T) {
  function get(): T {
    try {
      const stored = localStorage.getItem(key);
      return values.find(v => v === stored) ?? defaultValue;
    } catch {
      return defaultValue;
    }
  }

  function set(value: T): void {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Without storage the preference can't be changed, so the default stays
    }
    window.dispatchEvent(new Event(CHANGE_EVENT));
  }

  function use(): T {
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

export const PAGE_WIDTHS = ['normal', 'wide', 'full'] as const;
export type PageWidth = typeof PAGE_WIDTHS[number];
export const PAGE_WIDTH_LABELS: Record<PageWidth, string> = { normal: 'Normal', wide: 'Wide', full: 'Full Screen' };
/** How wide pages of a single column, such as the review page, are on large screens. Pages with
    a list beside a detail panel already use the full width. Phones use their full width whatever
    it is. Also read by the inline script in index.html, which applies it before the first paint. */
const pageWidth = choicePreference<PageWidth>('spares_page_width', PAGE_WIDTHS, 'normal');
export const usePageWidth = pageWidth.use;

/** Sets `data-page-width` on the root element, which index.css uses to pick `--page-max-width`. */
export function setPageWidth(width: PageWidth): void {
  pageWidth.set(width);
  document.documentElement.dataset.pageWidth = width;
}
