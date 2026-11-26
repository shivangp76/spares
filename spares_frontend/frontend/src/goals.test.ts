import { describe, expect, test } from "bun:test";

import { formatMinutes, reviewQueryPath } from "./goals";

describe("formatMinutes", () => {
  test("reads as hours and minutes", () => {
    expect(formatMinutes(0.2)).toBe("0 min");
    expect(formatMinutes(25.4)).toBe("25 min");
    expect(formatMinutes(120)).toBe("2 h");
    expect(formatMinutes(130)).toBe("2 h 10 min");
  });
});

describe("reviewQueryPath", () => {
  test("escapes the query and starts the review", () => {
    const query =
      '(tag="probability:grad" or tag=spdes) (tag="spdes:reading-course:chapter-3")';
    const path = reviewQueryPath(query);
    expect(path.endsWith("&start")).toBe(true);
    const params = new URL(path, "http://app.test").searchParams;
    expect(params.get("query")).toBe(query);
    expect(params.has("start")).toBe(true);
    expect(reviewQueryPath("a & b=c")).toBe(
      "/review?query=a%20%26%20b%3Dc&start",
    );
  });
});
