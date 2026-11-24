import { describe, expect, test } from "bun:test";
import {
  type Command,
  fuzzyScore,
  looksLikeNoteId,
  matchCommands,
  startsAWord,
} from "./commandPalette";

const command = (label: string): Command => ({
  id: label,
  label,
  run: () => {},
});

describe("fuzzyScore", () => {
  test("matches the query's characters in order", () => {
    expect(fuzzyScore("Review", "rv")).not.toBeNull();
    expect(fuzzyScore("Review", "vr")).toBeNull();
    expect(fuzzyScore("Review", "")).toBe(0);
  });

  test("ignores case and spaces in the query", () => {
    expect(fuzzyScore("Go to Statistics", "go STAT")).not.toBeNull();
  });

  test("prefers the start of the label and of words", () => {
    expect(fuzzyScore("Go to Notes", "n")!).toBeGreaterThan(
      fuzzyScore("Go to Cards", "n") ?? -Infinity,
    );
    expect(fuzzyScore("Notes", "no")!).toBeGreaterThan(
      fuzzyScore("Go to Notes", "no")!,
    );
  });
});

describe("matchCommands", () => {
  test("drops what doesn't match and puts the best first", () => {
    const commands = ["Go to Cards", "Go to Notes", "New note", "Sync"].map(
      command,
    );
    expect(matchCommands(commands, "note").map((c) => c.label)).toEqual([
      "New note",
      "Go to Notes",
    ]);
  });

  test("keeps the given order for an empty query", () => {
    const commands = ["b", "a", "c"].map(command);
    expect(matchCommands(commands, "").map((c) => c.label)).toEqual([
      "b",
      "a",
      "c",
    ]);
  });
});

describe("looksLikeNoteId", () => {
  test("accepts letters only", () => {
    expect(looksLikeNoteId("gir")).toBe(true);
    expect(looksLikeNoteId(" aaa ")).toBe(true);
    expect(looksLikeNoteId("tag=a")).toBe(false);
    expect(looksLikeNoteId("12")).toBe(false);
    expect(looksLikeNoteId("")).toBe(false);
  });
});

describe("startsAWord", () => {
  test("is true where the query begins a word of the label", () => {
    expect(startsAWord("Go to Statistics", "stat")).toBe(true);
    expect(startsAWord("New note", "new note")).toBe(true);
    expect(startsAWord("Go to Statistics", "tist")).toBe(false);
    expect(startsAWord("Go to Notes", "")).toBe(false);
  });
});
