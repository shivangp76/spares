import { expect, test } from 'bun:test';
import { SwipeTracker, ignoresSwipe, swipeRating, type PointerLike, type SwipeTrackerOptions } from './swipe';
import type { Rating } from './types/spares';

const WIDTH = 400;
const HEIGHT = 800;

function tracker(options: Partial<SwipeTrackerOptions> = {}) {
  return new SwipeTracker({
    width: () => WIDTH,
    height: () => HEIGHT,
    canSwipeUp: () => true,
    ignores: () => false,
    ...options,
  });
}

const at = (x: number, y: number, more: Partial<PointerLike> = {}): PointerLike => ({
  pointerId: 1,
  pointerType: 'touch',
  clientX: x,
  clientY: y,
  target: null,
  ...more,
});

/** Swipes from (200, 400) by (dx, dy) in a few moves, and lets go. */
function swipe(t: SwipeTracker, dx: number, dy: number) {
  expect(t.down(at(200, 400))).toBe(true);
  for (const f of [0.25, 0.5, 1]) t.move(at(200 + dx * f, 400 + dy * f));
  return t.up(at(200 + dx, 400 + dy));
}

test('past 30% of the width, left rates Again and right rates Good', () => {
  expect(swipe(tracker(), -0.35 * WIDTH, 5)).toBe('left');
  expect(swipe(tracker(), 0.35 * WIDTH, -5)).toBe('right');
});

test('short of the threshold the card snaps back', () => {
  const t = tracker();
  t.down(at(200, 400));
  const offset = t.move(at(200 + 0.2 * WIDTH, 400));
  expect(offset?.direction).toBe('right');
  expect(offset?.progress).toBeCloseTo(2 / 3);
  expect(t.up(at(200 + 0.2 * WIDTH, 400))).toBeNull();
  expect(t.tracking).toBe(false);
});

test('dragging past the threshold and back snaps back', () => {
  const t = tracker();
  t.down(at(200, 400));
  t.move(at(50, 400));
  expect(t.up(at(190, 400))).toBeNull();
});

test('swiping up rates Easy on a card that fits', () => {
  expect(swipe(tracker(), 0, -0.4 * HEIGHT)).toBe('up');
});

test('swiping up on a card that scrolls does nothing, leaving it to scroll', () => {
  const t = tracker({ canSwipeUp: () => false });
  t.down(at(200, 400));
  expect(t.move(at(200, 300))).toBeNull();
  expect(t.tracking).toBe(false);
  expect(t.up(at(200, 50))).toBeNull();
  // Sideways still rates
  expect(swipe(t, -200, 0)).toBe('left');
});

test('swiping down never rates', () => {
  const t = tracker();
  t.down(at(200, 100));
  expect(t.move(at(200, 200))).toBeNull();
  expect(t.up(at(200, 700))).toBeNull();
});

test('a tap or a wobble picks no direction', () => {
  const t = tracker();
  t.down(at(200, 400));
  expect(t.move(at(204, 403))?.direction).toBeNull();
  expect(t.up(at(204, 403))).toBeNull();
});

test('a mouse drag is left for selecting text', () => {
  const t = tracker();
  expect(t.down(at(200, 400, { pointerType: 'mouse' }))).toBe(false);
  expect(t.up(at(0, 400, { pointerType: 'mouse' }))).toBeNull();
});

test('a second finger is ignored', () => {
  const t = tracker();
  t.down(at(200, 400));
  expect(t.down(at(100, 100, { pointerId: 2 }))).toBe(false);
  expect(t.move(at(0, 400, { pointerId: 2 }))).toBeNull();
  expect(t.up(at(0, 400))).toBe('left');
});

test('a swipe in an ignored area does nothing', () => {
  const t = tracker({ ignores: target => target === 'input' as unknown as EventTarget });
  expect(t.down(at(200, 400, { target: 'input' as unknown as EventTarget }))).toBe(false);
  expect(t.up(at(0, 400))).toBeNull();
});

test('the browser cancelling the gesture rates nothing', () => {
  const t = tracker();
  t.down(at(200, 400));
  t.move(at(0, 400));
  t.cancel();
  expect(t.up(at(0, 400))).toBeNull();
});

/** A stand-in for an element, with what `ignoresSwipe` reads. */
function element(matches: string[], parent: unknown = null, scrolls = false, overflowX = 'visible') {
  const el = {
    parentElement: parent,
    scrollWidth: scrolls ? 500 : 100,
    clientWidth: 100,
    ownerDocument: { defaultView: { getComputedStyle: () => ({ overflowX }) } },
    closest: (selector: string) => (matches.some(m => selector.includes(m)) ? el : null),
  };
  return el as unknown as Element;
}

test('inputs, typed answers and marked areas are left alone', () => {
  const card = element([]);
  expect(ignoresSwipe(element(['input']), card)).toBe(true);
  expect(ignoresSwipe(element(['textarea']), card)).toBe(true);
  expect(ignoresSwipe(element(['[data-no-swipe]']), card)).toBe(true);
  expect(ignoresSwipe(element([], card), card)).toBe(false);
  expect(ignoresSwipe(null, card)).toBe(false);
});

test('something that scrolls sideways, like a PDF or an image being panned, keeps the swipe', () => {
  const card = element([]);
  const wide = element([], card, true, 'auto');
  expect(ignoresSwipe(element([], wide), card)).toBe(true);
  // Overflowing but clipped, so nothing to pan
  const clipped = element([], card, true, 'hidden');
  expect(ignoresSwipe(element([], clipped), card)).toBe(false);
});

const rating = (id: number, description: string) => ({ id, description }) as Rating;

test('left is Again, right Good and up Easy, by name', () => {
  const fsrs = [rating(1, 'Again'), rating(2, 'Hard'), rating(3, 'Good'), rating(4, 'Easy')];
  expect(swipeRating(fsrs, 'left')?.id).toBe(1);
  expect(swipeRating(fsrs, 'right')?.id).toBe(3);
  expect(swipeRating(fsrs, 'up')?.id).toBe(4);
});

test('with two ratings, up rates nothing rather than Good again', () => {
  const two = [rating(1, 'Fail'), rating(2, 'Pass')];
  expect(swipeRating(two, 'left')?.id).toBe(1);
  expect(swipeRating(two, 'right')?.id).toBe(2);
  expect(swipeRating(two, 'up')).toBeUndefined();
});
