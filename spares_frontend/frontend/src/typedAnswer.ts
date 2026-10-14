// A port of the comparison of a typed answer with its expected answer in
// `spares_core/src/parsers/typed_answer.rs`, so it also works offline. Both run the cases in
// `spares_core/tests/fixtures/typed_answer_cases.json` (see `typedAnswer.test.ts`).

export type SegmentKind = 'good' | 'bad' | 'missed';

export interface DiffSegment {
  kind: SegmentKind;
  text: string;
}

/** The typed answer (`good` and `bad`) and the expected answer (`good` and `missed`). */
export interface TypedDiff {
  typed: DiffSegment[];
  expected: DiffSegment[];
}

/** Above this many character pairs, a mismatch is not diffed, but shown as wholly wrong. */
const MAX_DIFF_CELLS = 1_000_000;

function push(segments: DiffSegment[], kind: SegmentKind, c: string) {
  const last = segments[segments.length - 1];
  if (last && last.kind === kind) last.text += c;
  else segments.push({ kind, text: c });
}

/**
 * Compares `typed` with `expected` character by character. Segments hold the characters as typed
 * or expected, even where case or combining marks are ignored.
 */
export function compareTypedAnswer(expected: string, typed: string, ignoreCase: boolean, ignoreCombining: boolean): TypedDiff {
  const key = (c: string): string => {
    let k = ignoreCombining ? c.normalize('NFKD').replace(/\p{M}/gu, '') : c;
    if (ignoreCase) k = k.toLowerCase();
    return k;
  };
  // By code point, like Rust's `chars()`
  const e = Array.from(expected.trim().normalize('NFC'));
  const t = Array.from(typed.trim().normalize('NFC'));
  const ek = e.map(key);
  const tk = t.map(key);
  const n = e.length;
  const m = t.length;
  const diff: TypedDiff = { typed: [], expected: [] };

  if (n * m > MAX_DIFF_CELLS) {
    const same = ek.length === tk.length && ek.every((k, i) => k === tk[i]);
    for (const c of e) push(diff.expected, same ? 'good' : 'missed', c);
    for (const c of t) push(diff.typed, same ? 'good' : 'bad', c);
    return diff;
  }

  // `lcs[i * (m + 1) + j]` is the length of the longest common subsequence of `e[i..]` and `t[j..]`
  const lcs = new Uint32Array((n + 1) * (m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * (m + 1) + j] = ek[i] === tk[j]
        ? lcs[(i + 1) * (m + 1) + j + 1] + 1
        : Math.max(lcs[(i + 1) * (m + 1) + j], lcs[i * (m + 1) + j + 1]);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (ek[i] === tk[j]) {
      push(diff.expected, 'good', e[i++]);
      push(diff.typed, 'good', t[j++]);
    } else if (lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
      push(diff.expected, 'missed', e[i++]);
    } else {
      push(diff.typed, 'bad', t[j++]);
    }
  }
  while (i < n) push(diff.expected, 'missed', e[i++]);
  while (j < m) push(diff.typed, 'bad', t[j++]);
  return diff;
}

/** Whether the typed answer matches the expected answer. */
export function isExact(diff: TypedDiff): boolean {
  return [...diff.typed, ...diff.expected].every(s => s.kind === 'good');
}
