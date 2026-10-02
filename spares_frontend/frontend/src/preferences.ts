import { useSyncExternalStore } from 'react';

const SHOW_REVIEW_TIMER_KEY = 'spares_show_review_timer';
const CHANGE_EVENT = 'spares-preferences-change';

/** Whether the review page shows the running recall and rate timers. Shown unless turned off. */
export function getShowReviewTimer(): boolean {
  try {
    return localStorage.getItem(SHOW_REVIEW_TIMER_KEY) !== 'false';
  } catch {
    // Without storage the preference can't be remembered, so the default is used
    return true;
  }
}

export function setShowReviewTimer(show: boolean): void {
  try {
    if (show) localStorage.removeItem(SHOW_REVIEW_TIMER_KEY);
    else localStorage.setItem(SHOW_REVIEW_TIMER_KEY, 'false');
  } catch {
    // Without storage the preference can't be changed, so the timer stays shown
  }
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, onChange);
  return () => window.removeEventListener(CHANGE_EVENT, onChange);
}

export function useShowReviewTimer(): boolean {
  return useSyncExternalStore(subscribe, getShowReviewTimer);
}
