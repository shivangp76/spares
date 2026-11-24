/** Where the cursor is in the parts of a Typst note that name other notes, for `noteCompletion`. */

/** The keyword being written in `#lin[…]` before the cursor, and where it starts. */
export function linkAt(
  before: string,
): { from: number; text: string } | null {
  const match = /#lin\(?\[([^[\]]*)$/.exec(before);
  return match ? { from: before.length - match[1].length, text: match[1] } : null;
}

/**
 * The note named after `inh:` or `mv:` before the cursor: the text typed for it, or, after its
 * `/`, its id and the order typed so far.
 */
export function inheritAt(
  before: string,
):
  | { kind: "note"; from: number; text: string }
  | { kind: "order"; from: number; noteId: string; text: string }
  | null {
  const order = /(?:^|[\s[,;])(?:inh|mv):([a-z]+)\/(\d*)$/i.exec(before);
  if (order)
    return {
      kind: "order",
      from: before.length - order[2].length,
      noteId: order[1].toLowerCase(),
      text: order[2],
    };
  // The note's id, or words of its text, which may have spaces
  const note = /(?:^|[\s[,;])(?:inh|mv):([^[\],;/]*)$/i.exec(before);
  return note
    ? { kind: "note", from: before.length - note[1].length, text: note[1] }
    : null;
}
