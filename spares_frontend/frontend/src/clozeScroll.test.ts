import { expect, test } from 'bun:test';
import { clozeScrollDelta } from './clozeScroll';

// Clozes are centered 400 down this view
const view = { top: 0, bottom: 600 };

test('clozes already in view are not scrolled to', () => {
  expect(clozeScrollDelta([{ top: 10, bottom: 30 }, { top: 500, bottom: 520 }], view)).toBeNull();
  expect(clozeScrollDelta([], view)).toBeNull();
});

test('a cloze out of view is centered two thirds down', () => {
  expect(clozeScrollDelta([{ top: 900, bottom: 920 }], view)).toBe(510);
  expect(clozeScrollDelta([{ top: -320, bottom: -300 }], view)).toBe(-710);
});

test('a cloze partly out of view is centered two thirds down', () => {
  expect(clozeScrollDelta([{ top: 590, bottom: 610 }], view)).toBe(200);
});

test('clozes that fit together are centered together', () => {
  expect(clozeScrollDelta([{ top: 700, bottom: 720 }, { top: 800, bottom: 820 }], view)).toBe(360);
});

test('clozes centered two thirds down are kept in view', () => {
  expect(clozeScrollDelta([{ top: 700, bottom: 720 }, { top: 1100, bottom: 1120 }], view)).toBe(520);
});

test('clozes that do not fit together center the first', () => {
  expect(clozeScrollDelta([{ top: 700, bottom: 720 }, { top: 1400, bottom: 1420 }], view)).toBe(310);
});

test('a cloze taller than the view is shown from its top', () => {
  expect(clozeScrollDelta([{ top: 700, bottom: 1400 }], { top: 50, bottom: 500 })).toBe(650);
});

test('the view can be inset', () => {
  expect(clozeScrollDelta([{ top: 590, bottom: 610 }], { top: 0, bottom: 300 })).toBe(400);
});
