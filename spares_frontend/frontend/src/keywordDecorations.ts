import {
  Decoration,
  RangeSet,
  RangeValue,
  type Range,
  type Text,
  type ChangeSet,
} from "@uiw/react-codemirror";
import { formatNoteId } from "./utils";

export function keywordOwners(list: [number, string][]): Map<string, number[]> {
  const owners = new Map<string, number[]>();
  for (const [id, keyword] of list) {
    const key = keyword.trim().toLowerCase();
    const ids = owners.get(key);
    if (ids) ids.push(id);
    else owners.set(key, [id]);
  }
  return owners;
}

// Track incomplete and unmarked expressions too. Editing a later line or adding
// a closing bracket can turn either into a duplicate without changing its opener.
class Keyword extends RangeValue {
  constructor(
    readonly prefix: number,
    readonly mark: Decoration | null,
  ) {
    super();
  }
}

export type KeywordRanges = RangeSet<Keyword>;
const MAX_PREFIX = "#key([".length;

/** Read expressions whose opener starts in [from, to], including multiline values. */
function scan(
  doc: Text,
  from: number,
  to: number,
  owners: Map<string, number[]>,
  noteId: number | null,
) {
  const ranges: Range<Keyword>[] = [];
  const text = doc.sliceString(from, Math.min(to + MAX_PREFIX, doc.length));
  for (const match of text.matchAll(/#key\(?\[/g)) {
    const start = from + match.index;
    if (start > to) break;
    let end = start + match[0].length;
    const valueFrom = end;
    let mark: Decoration | null = null;
    for (const chunk of doc.iterRange(valueFrom)) {
      const bracket = chunk.search(/[[\]]/);
      if (bracket < 0) {
        end += chunk.length;
        continue;
      }
      end += bracket + 1;
      if (chunk[bracket] === "]" && end > valueFrom + 1) {
        const key = doc
          .sliceString(valueFrom, end - 1)
          .trim()
          .toLowerCase();
        const ids = owners.get(key)?.filter((id) => id !== noteId);
        if (ids?.length)
          mark = Decoration.mark({
            class: "cm-duplicate-keyword",
            attributes: {
              title: `Note ${ids.map(formatNoteId).join(", ")} already has this keyword, so links to it find both`,
            },
          });
      }
      break;
    }
    ranges.push(new Keyword(match[0].length, mark).range(start, end));
  }
  return ranges;
}

export function keywordRanges(
  doc: Text,
  owners: Map<string, number[]>,
  noteId: number | null,
  previous?: { ranges: KeywordRanges; changes: ChangeSet },
): KeywordRanges {
  if (!owners.size) return RangeSet.empty;
  if (!previous) return RangeSet.of(scan(doc, 0, doc.length, owners, noteId));
  const { changes, ranges } = previous;
  const regions: { from: number; to: number }[] = [];
  changes.iterChangedRanges((start, end) => {
    // A changed character can complete a prefix up to five positions earlier.
    let from = Math.max(0, start - MAX_PREFIX + 1);
    ranges.between(start, end, (opener) => {
      from = Math.min(from, opener);
    });
    const last = regions[regions.length - 1];
    if (last && last.to >= from) last.to = end;
    else regions.push({ from, to: end });
  });
  let kept = ranges;
  const added: Range<Keyword>[] = [];
  for (const { from, to } of regions) {
    kept = kept.update({
      filterFrom: from,
      filterTo: to,
      filter: (start) => start < from || start > to,
    });
    for (const keyword of scan(
      doc,
      changes.mapPos(from, -1),
      changes.mapPos(to, 1),
      owners,
      noteId,
    )) {
      added.push(keyword);
    }
  }
  return kept.map(changes).update({ add: added, sort: true });
}

export function visibleKeywordMarks(
  ranges: KeywordRanges,
  from: number,
  to: number,
) {
  const marks: Range<Decoration>[] = [];
  ranges.between(from, to, (start, end, value) => {
    if (value.mark) marks.push(value.mark.range(start + value.prefix, end - 1));
  });
  return Decoration.set(marks, true);
}
