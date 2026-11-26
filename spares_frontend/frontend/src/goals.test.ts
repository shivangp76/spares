import { describe, expect, test } from "bun:test";

import { reviewQueryPath } from "./goals";

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
