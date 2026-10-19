import { expect, test } from 'bun:test';
import { keyName } from './keys';

const press = (key: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }> = {}) =>
  keyName({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });

test('a character names itself, which already says Shift', () => {
  expect(press('j')).toBe('j');
  expect(press('S', { shiftKey: true })).toBe('S');
  expect(press('#', { shiftKey: true })).toBe('#');
});

test('a named key says Shift', () => {
  expect(press('ArrowDown')).toBe('ArrowDown');
  expect(press('ArrowDown', { shiftKey: true })).toBe('Shift+ArrowDown');
});

test('Ctrl and Cmd are both Ctrl', () => {
  expect(press('a', { ctrlKey: true })).toBe('Ctrl+a');
  expect(press('a', { metaKey: true })).toBe('Ctrl+a');
  expect(press('A', { metaKey: true, shiftKey: true })).toBe('Ctrl+A');
});

test('Space is named', () => {
  expect(press(' ')).toBe('Space');
});

test('no key is named with Alt', () => {
  expect(press('j', { altKey: true })).toBeNull();
});
