import { describe, expect, test } from "bun:test";

import {
  displayValue,
  draftValues,
  errorLine,
  formatDuration,
  parseDuration,
  parseInput,
  parseSteps,
  valueAt,
} from "./collectionSettings";

describe("durations", () => {
  test("read and write as the largest units", () => {
    expect(formatDuration(60)).toBe("1m");
    expect(formatDuration(5400)).toBe("1h30m");
    expect(formatDuration(90)).toBe("1m30s");
    expect(formatDuration(86_400 * 2)).toBe("2d");
    expect(parseDuration("1h30m")).toBe(5400);
    expect(parseDuration("90")).toBe(90);
    expect(parseDuration("1.5h")).toBe(5400);
    expect(parseDuration("soon")).toBeNull();
  });

  test("steps are durations separated by spaces or commas", () => {
    expect(parseSteps("1m 10m, 1h")).toEqual([60, 600, 3600]);
    expect(parseSteps("")).toEqual([]);
    expect(parseSteps("1m later")).toBe(
      '"later" is not a duration, such as 30s, 10m, 1h or 1d',
    );
    expect(parseSteps("0m")).toContain("not a duration");
  });
});

describe("fields", () => {
  test("show and parse their values", () => {
    expect(displayValue({ type: "steps" }, [60, 600])).toBe("1m 10m");
    expect(displayValue({ type: "duration", unit: "days" }, 15_552_000)).toBe(
      "180",
    );
    expect(displayValue({ type: "limit" }, 4_294_967_295)).toBe("");
    expect(displayValue({ type: "limit" }, 200)).toBe("200");
    expect(parseInput({ type: "limit" }, "")).toEqual({ remove: true });
    expect(parseInput({ type: "number", min: 1 }, "0")).toEqual({
      error: "At least 1",
    });
    expect(parseInput({ type: "duration", unit: "minutes" }, "20")).toEqual({
      value: 1200,
    });
    expect(
      parseInput({ type: "duration", unit: "minutes", optional: true }, ""),
    ).toEqual({
      remove: true,
    });
    expect(
      parseInput({ type: "select", options: [] }, '"unless_blocking"'),
    ).toEqual({
      value: "unless_blocking",
    });
  });

  test("values are found by path", () => {
    expect(
      valueAt({ queue: { review_order: "due" } }, ["queue", "review_order"]),
    ).toBe("due");
    expect(valueAt({ queue: null }, ["queue", "review_order"])).toBeUndefined();
  });
});

test("a removed table's own changes aren't sent, and it goes first", () => {
  expect(
    draftValues([
      { path: ["new_cards_daily_limit"], value: 10, remove: false },
      { path: ["queue", "presets", "a", "tags"], value: ["x"], remove: false },
      { path: ["queue", "presets", "a"], value: undefined, remove: true },
    ]),
  ).toEqual([
    { key: ["queue", "presets", "a"] },
    { key: ["new_cards_daily_limit"], value: 10 },
  ]);
});

test("an error's line is found", () => {
  expect(errorLine("Invalid config: the settings line 3: invalid type")).toBe(
    3,
  );
  expect(errorLine("no line here")).toBeNull();
});
