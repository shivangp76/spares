// Scrolls a card's cloze into view when the card is longer than the screen, so it is clear where
// the cloze is.

/** A box's top and bottom, in the window's viewport coordinates. */
export interface VerticalSpan {
  top: number;
  bottom: number;
}

/**
 * Where in the view a cloze is centered, as a fraction of its height from the top. Below the
 * middle, since what comes before a cloze is usually more useful context than what comes after.
 */
const CLOZE_POSITION = 2 / 3;

/**
 * How far to scroll down (negative for up) to show the clozes in `view`, or `null` if they are
 * all already in it. They are centered at `CLOZE_POSITION`, as far as they still fit, or if they
 * do not fit together, the first one is.
 */
export function clozeScrollDelta(clozes: VerticalSpan[], view: VerticalSpan): number | null {
  if (clozes.length === 0) return null;
  const top = Math.min(...clozes.map(c => c.top));
  const bottom = Math.max(...clozes.map(c => c.bottom));
  if (top >= view.top && bottom <= view.bottom) return null;
  const viewHeight = view.bottom - view.top;
  const target = bottom - top <= viewHeight ? { top, bottom } : clozes[0];
  // A cloze taller than the view is shown from its top
  if (target.bottom - target.top > viewHeight) return target.top - view.top;
  const height = target.bottom - target.top;
  const anchor = view.top + viewHeight * CLOZE_POSITION;
  const newTop = Math.min(Math.max(anchor - height / 2, view.top), view.bottom - height);
  return target.top - newTop;
}

/**
 * Scrolls the window to show the clozes, given in reading order. The part of the window covered
 * by pinned bars is left out using the root's `scroll-padding` (see `index.css`).
 */
export function scrollClozesIntoView(clozes: VerticalSpan[]) {
  const root = getComputedStyle(document.documentElement);
  const view = {
    top: parseFloat(root.scrollPaddingTop) || 0,
    bottom: window.innerHeight - (parseFloat(root.scrollPaddingBottom) || 0),
  };
  const delta = clozeScrollDelta(clozes, view);
  if (delta !== null) window.scrollBy({ top: delta });
}
