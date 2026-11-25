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
