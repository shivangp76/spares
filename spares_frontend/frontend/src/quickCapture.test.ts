import { expect, test } from 'bun:test';
import { searchedTag } from './quickCapture';

test('the tag a search is only for', () => {
  expect(searchedTag('tag=math')).toBe('math');
  expect(searchedTag(' tag = "math:linear algebra" ')).toBe('math:linear algebra');
  expect(searchedTag('tag=math c.due<1')).toBeNull();
  expect(searchedTag('physics')).toBeNull();
  expect(searchedTag(null)).toBeNull();
});
