import { expect, test } from "bun:test";
import {
  EditorState,
  type ChangeSpec,
  type DecorationSet,
} from "@uiw/react-codemirror";
import {
  keywordRanges,
  visibleKeywordMarks,
  keywordOwners,
} from "./keywordDecorations";

function ranges(set: DecorationSet) {
  const result = [];
  for (let cur = set.iter(); cur.value; cur.next()) {
    result.push([cur.from, cur.to, cur.value.spec.attributes.title]);
  }
  return result;
}

test("incremental marks agree with a full scan across bracket, prefix and multiline edits", () => {
  let state = EditorState.create({
    doc: "#key[ One ]\n#key([two\nlines]) #key[one] #key[self]",
  });
  const owners = keywordOwners([
    [1, "ONE"],
    [2, "one"],
    [3, "two\nlines"],
    [1, "self"],
  ]);
  let candidates = keywordRanges(state.doc, owners, 1);
  expect(
    ranges(visibleKeywordMarks(candidates, 0, state.doc.length)).map(
      ([from, to]) => [from, to],
    ),
  ).toEqual([
    [5, 10],
    [18, 27],
    [35, 38],
  ]);
  function edit(changes: ChangeSpec) {
    const tx = state.update({ changes });
    candidates = keywordRanges(tx.newDoc, owners, 1, {
      ranges: candidates,
      changes: tx.changes,
    });
    expect(
      ranges(visibleKeywordMarks(candidates, 0, tx.newDoc.length)),
    ).toEqual(
      ranges(
        visibleKeywordMarks(
          keywordRanges(tx.newDoc, owners, 1),
          0,
          tx.newDoc.length,
        ),
      ),
    );
    state = tx.state;
  }
  edit([
    { from: 0, to: 1 },
    { from: 24, insert: "]" },
    { from: 39, insert: "\n" },
  ]);
  edit([
    { from: 0, insert: "#" },
    { from: 23, to: 24 },
  ]);
  // Paste, delete and replace at every boundary, including a closing bracket at EOF.
  for (const insert of ["", "]", "#key([one]", "[two\nlines]", "\n"]) {
    const length = state.doc.length;
    for (let from = 0; from <= length; from++) {
      const old = state.doc.sliceString(from, Math.min(length, from + 2));
      edit({ from, to: from + old.length, insert });
      edit({ from, to: from + insert.length, insert: old });
    }
  }
});

test("visible matches follow the original syntax through simultaneous edits", () => {
  const list: [number, string][] = [
    [1, "one"],
    [2, "two\nlines"],
  ];
  const owners = keywordOwners(list);
  let state = EditorState.create({ doc: "#key[one]\n".repeat(75) });
  let candidates = keywordRanges(state.doc, owners, null);
  let seed = 1;
  const next = (max: number) => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed % max;
  };
  const inserts = [
    "#key[one]",
    "#key([two\nlines]",
    "#key",
    "one",
    "[",
    "]",
    "",
    "\n",
  ];
  for (let i = 0; i < 500; i++) {
    const starts = [
      next(state.doc.length + 1),
      next(state.doc.length + 1),
    ].sort((a, b) => a - b);
    const changes = starts.map((from, j) => ({
      from,
      to: Math.min(from + next(8), starts[j + 1] ?? state.doc.length),
      insert: inserts[next(inserts.length)],
    }));
    const tx = state.update({ changes });
    candidates = keywordRanges(tx.newDoc, owners, null, {
      ranges: candidates,
      changes: tx.changes,
    });
    const expected = [];
    for (const m of tx.newDoc.toString().matchAll(/#key\(?\[([^[\]]+)\]/g)) {
      if (!owners.has(m[1].trim().toLowerCase())) continue;
      const start = m.index + m[0].indexOf("[") + 1;
      expected.push([start, start + m[1].length]);
    }
    expect(
      ranges(visibleKeywordMarks(candidates, 0, tx.newDoc.length)).map(
        ([a, b]) => [a, b],
      ),
    ).toEqual(expected);
    state = tx.state;
  }
});
