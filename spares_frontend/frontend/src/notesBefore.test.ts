import { expect, test } from "bun:test";
import { parseNoteId } from "./utils";

// Imported after the test runner is set up; ask.ts only reaches the dialog host when asked
const { notesBeforeQuery } = await import("./notesBefore");

test("notesBeforeQuery writes the manual's relative-date query", () => {
  expect(notesBeforeQuery(parseNoteId("gir")!, 5)).toBe(
    "created_at<=note[gir].created_at and created_at>=note[gir].created_at-5d",
  );
});
