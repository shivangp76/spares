import { expect, test } from 'bun:test';
import cases from '../../../spares_core/tests/fixtures/typed_answer_cases.json';
import { compareTypedAnswer, isExact, type TypedDiff } from './typedAnswer';

interface Case {
  name: string;
  expected: string;
  typed: string;
  ignore_case: boolean;
  ignore_combining: boolean;
  diff: TypedDiff;
}

for (const c of cases as Case[]) {
  test(c.name, () => {
    const diff = compareTypedAnswer(c.expected, c.typed, c.ignore_case, c.ignore_combining);
    expect(diff).toEqual(c.diff);
    expect(isExact(diff)).toBe([...c.diff.typed, ...c.diff.expected].every(s => s.kind === 'good'));
  });
}

test('a long mismatch is not diffed', () => {
  const expected = 'a'.repeat(2000);
  const typed = `${'a'.repeat(1999)}b`;
  expect(compareTypedAnswer(expected, typed, false, false)).toEqual({
    typed: [{ kind: 'bad', text: typed }],
    expected: [{ kind: 'missed', text: expected }],
  });
  expect(isExact(compareTypedAnswer(expected, expected, false, false))).toBe(true);
});
