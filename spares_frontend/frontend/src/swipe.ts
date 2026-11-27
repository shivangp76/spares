/**
 * Swiping a card to rate it, on touch screens: left for Again, right for Good, and up for Easy, but
 * up only on a card that fits on the screen, since on a longer one swiping up scrolls it. The card
 * follows the finger and snaps back when let go short of the threshold, so a swipe is never an
 * accident. Mouse drags are left alone, for selecting text.
 *
 * `SwipeTracker` is the gesture alone, fed pointer events, so it is tested without a browser.
 * `useSwipeToRate` wires it to the card and the iframes Markdown cards are shown in.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import type { Rating } from './types/spares';

export type SwipeDirection = 'left' | 'right' | 'up';

/** How far a card is dragged before letting go rates it, as a fraction of its width (or, up, of the
    screen's height). */
export const SWIPE_THRESHOLD = 0.3;
/** How far a finger moves before the swipe picks its axis, so a tap or a wobble does nothing. */
const AXIS_SLOP = 10;

/** The parts of a `PointerEvent` the tracker reads, with its position on the screen. */
export interface PointerLike {
  pointerId: number;
  pointerType: string;
  clientX: number;
  clientY: number;
  target: EventTarget | null;
}

export interface SwipeTrackerOptions {
  /** The card's width, against which a sideways swipe's threshold is measured. */
  width: () => number;
  /** The height an upward swipe is measured against, the screen's. */
  height: () => number;
  /** Whether swiping up rates, i.e. the card fits on the screen. */
  canSwipeUp: () => boolean;
  /** Whether a swipe starting on `target` is left to it, e.g. an input or something that pans. */
  ignores: (target: EventTarget | null) => boolean;
}

/** Where the card is dragged to, and which rating letting go there would give. */
export interface SwipeOffset {
  x: number;
  y: number;
  /** The direction being dragged in, once the axis is picked. */
  direction: SwipeDirection | null;
  /** How far towards the threshold, from 0 to 1 (1 rates on letting go). */
  progress: number;
}

export const NO_OFFSET: SwipeOffset = { x: 0, y: 0, direction: null, progress: 0 };

/** One finger's swipe at a time, from `down` to `up` or `cancel`. */
export class SwipeTracker {
  private start: { id: number; x: number; y: number } | null = null;
  private axis: 'x' | 'y' | null = null;
  private offset: SwipeOffset = NO_OFFSET;

  constructor(private readonly options: SwipeTrackerOptions) {}

  /** Starts tracking a finger or pen. Returns whether it is tracked. */
  down(e: PointerLike): boolean {
    if (this.start || e.pointerType === 'mouse' || this.options.ignores(e.target)) return false;
    this.start = { id: e.pointerId, x: e.clientX, y: e.clientY };
    this.axis = null;
    this.offset = NO_OFFSET;
    return true;
  }

  /** Where the card is dragged to, or null when this pointer isn't swiping (any longer). */
  move(e: PointerLike): SwipeOffset | null {
    const start = this.start;
    if (!start || e.pointerId !== start.id) return null;
    const dx = e.clientX - start.x;
    const dy = e.clientY - start.y;
    if (this.axis === null) {
      if (Math.hypot(dx, dy) < AXIS_SLOP) return NO_OFFSET;
      this.axis = Math.abs(dx) >= Math.abs(dy) ? 'x' : 'y';
      // Down, or up on a card that scrolls, is the page's to scroll
      if (this.axis === 'y' && (dy > 0 || !this.options.canSwipeUp())) {
        this.cancel();
        return null;
      }
    }
    if (this.axis === 'x') {
      const width = Math.max(1, this.options.width());
      this.offset = {
        x: dx,
        y: 0,
        direction: dx < 0 ? 'left' : 'right',
        progress: Math.min(1, Math.abs(dx) / (width * SWIPE_THRESHOLD)),
      };
    } else {
      const up = Math.max(0, -dy);
      const height = Math.max(1, this.options.height());
      this.offset = { x: 0, y: -up, direction: 'up', progress: Math.min(1, up / (height * SWIPE_THRESHOLD)) };
    }
    return this.offset;
  }

  /** Lets go: the direction to rate in, or null to snap back. */
  up(e: PointerLike): SwipeDirection | null {
    if (!this.start || e.pointerId !== this.start.id) return null;
    this.move(e);
    const { direction, progress } = this.offset;
    this.cancel();
    return progress >= 1 ? direction : null;
  }

  /** Stops tracking without rating, e.g. when the browser takes the gesture to scroll. */
  cancel(): void {
    this.start = null;
    this.axis = null;
    this.offset = NO_OFFSET;
  }

  get tracking(): boolean {
    return this.start !== null;
  }
}

/** Elements a swipe starting inside is left to: fields to type in, and anything marked to pan. */
const IGNORED = 'input, textarea, select, [contenteditable=""], [contenteditable="true"], [data-no-swipe]';

/** Whether a swipe starting on `target` belongs to it rather than to the card: in a field, in an
    element marked `data-no-swipe` (an image being panned, a PDF), or in something that scrolls
    sideways, such as a wide code block or formula. */
export function ignoresSwipe(target: EventTarget | null, card: Element | null): boolean {
  // Not `instanceof Element`, which is false for an element in an iframe's document
  let el = target !== null && typeof (target as Element).closest === 'function' ? (target as Element) : null;
  if (el?.closest(IGNORED)) return true;
  for (; el && el !== card; el = el.parentElement) {
    if (el.scrollWidth > el.clientWidth + 1) {
      const overflow = el.ownerDocument.defaultView?.getComputedStyle(el).overflowX;
      if (overflow === 'auto' || overflow === 'scroll') return true;
    }
  }
  return false;
}

/**
 * Swiping `ref`'s card to rate it, while `enabled`. Returns where it is dragged to, for showing it
 * follow the finger, and whether swiping up rates, which the card's `touch-action` follows.
 *
 * Markdown cards are in same-origin iframes, whose pointer events never reach this document, so the
 * tracker listens in their documents too.
 */
export function useSwipeToRate(
  ref: React.RefObject<HTMLElement | null>,
  enabled: boolean,
  onSwipe: (direction: SwipeDirection) => void,
): { offset: SwipeOffset; canSwipeUp: boolean } {
  const [offset, setOffset] = useState<SwipeOffset>(NO_OFFSET);
  const [canSwipeUp, setCanSwipeUp] = useState(false);
  const canSwipeUpRef = useRef(false);
  const onSwipeRef = useRef(onSwipe);
  useEffect(() => {
    onSwipeRef.current = onSwipe;
  }, [onSwipe]);

  // Whether the card fits on the screen, so swiping up has nothing to scroll
  const measure = useCallback(() => {
    const el = ref.current;
    const fits = el !== null && el.getBoundingClientRect().height <= (window.visualViewport?.height ?? window.innerHeight);
    canSwipeUpRef.current = fits;
    setCanSwipeUp(fits);
  }, [ref]);

  useEffect(() => {
    const card = ref.current;
    if (!enabled || !card) return;
    const tracker = new SwipeTracker({
      width: () => card.getBoundingClientRect().width,
      height: () => window.visualViewport?.height ?? window.innerHeight,
      canSwipeUp: () => canSwipeUpRef.current,
      ignores: target => ignoresSwipe(target, card),
    });
    // Where the finger is on the screen. Not `clientX`, which in an iframe is relative to the
    // iframe, and so to the card as it follows the finger, which would halve how far it seems to go.
    const at = (e: PointerEvent): PointerLike => ({
      pointerId: e.pointerId,
      pointerType: e.pointerType,
      clientX: e.screenX,
      clientY: e.screenY,
      target: e.target,
    });
    const down = (e: PointerEvent) => {
      if (tracker.down(at(e))) setOffset(NO_OFFSET);
    };
    const move = (e: PointerEvent) => {
      if (!tracker.tracking) return;
      const next = tracker.move(at(e));
      setOffset(next ?? NO_OFFSET);
    };
    const up = (e: PointerEvent) => {
      if (!tracker.tracking) return;
      const direction = tracker.up(at(e));
      setOffset(NO_OFFSET);
      if (direction) onSwipeRef.current(direction);
    };
    const cancel = () => {
      tracker.cancel();
      setOffset(NO_OFFSET);
    };

    // The documents the card's pointer events land in: this one, and each iframe's in the card
    const listening = new Set<Document>();
    const listen = (doc: Document, root: EventTarget) => {
      if (listening.has(doc)) return;
      listening.add(doc);
      root.addEventListener('pointerdown', down as EventListener);
      doc.addEventListener('pointermove', move);
      doc.addEventListener('pointerup', up);
      doc.addEventListener('pointercancel', cancel);
    };
    const frames = () => {
      for (const iframe of card.querySelectorAll('iframe')) {
        const doc = iframe.contentDocument;
        if (doc?.documentElement && !listening.has(doc)) {
          // touch-action stops at the document, so the iframe's needs it too
          doc.documentElement.style.touchAction = canSwipeUpRef.current ? 'pinch-zoom' : 'pan-y pinch-zoom';
          listen(doc, doc);
        }
      }
    };
    listen(document, card);
    frames();
    // An iframe's load doesn't bubble, but is seen while capturing
    card.addEventListener('load', frames, true);
    const observer = new ResizeObserver(() => {
      measure();
      frames();
    });
    observer.observe(card);
    window.addEventListener('resize', measure);
    queueMicrotask(measure);
    return () => {
      card.removeEventListener('load', frames, true);
      observer.disconnect();
      window.removeEventListener('resize', measure);
      for (const doc of listening) {
        (doc === document ? card : doc).removeEventListener('pointerdown', down as EventListener);
        doc.removeEventListener('pointermove', move);
        doc.removeEventListener('pointerup', up);
        doc.removeEventListener('pointercancel', cancel);
        if (doc !== document) doc.documentElement.style.touchAction = '';
      }
      tracker.cancel();
      queueMicrotask(() => setOffset(NO_OFFSET));
    };
  }, [ref, enabled, measure]);

  // Keeps the iframes' touch-action in step once the card is measured
  useEffect(() => {
    const card = ref.current;
    if (!enabled || !card) return;
    for (const iframe of card.querySelectorAll('iframe')) {
      const root = iframe.contentDocument?.documentElement;
      if (root) root.style.touchAction = canSwipeUp ? 'pinch-zoom' : 'pan-y pinch-zoom';
    }
  }, [ref, enabled, canSwipeUp]);

  return { offset, canSwipeUp };
}

/** The rating a swipe gives: left the worst (Again), right Good, and up Easy, each by name where
    the scheduler has it, else by position. */
export function swipeRating(
  ratings: Rating[],
  direction: SwipeDirection,
): Rating | undefined {
  const named = (name: string) =>
    ratings.find((r) => r.description.toLowerCase() === name);
  if (direction === "left") return ratings[0];
  if (direction === "right")
    return named("good") ?? ratings[Math.min(2, ratings.length - 1)];
  const easy = named("easy") ?? ratings[ratings.length - 1];
  // Not the same rating as a swipe right, on a scheduler with only two or three
  return easy === swipeRating(ratings, "right") ? undefined : easy;
}
