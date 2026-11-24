import { describe, expect, test } from "bun:test";
import { isSameHost } from "./login";

describe("isSameHost", () => {
  const origin = "https://spares.example.com";

  test("the page's own address, with or without a path or trailing slash", () => {
    expect(isSameHost(origin, origin)).toBe(true);
    expect(isSameHost(`${origin}/`, origin)).toBe(true);
    expect(isSameHost(` ${origin}/api `, origin)).toBe(true);
  });

  test("another host, scheme or port", () => {
    expect(isSameHost("https://other.example.com", origin)).toBe(false);
    expect(isSameHost("http://spares.example.com", origin)).toBe(false);
    expect(isSameHost("https://spares.example.com:8443", origin)).toBe(false);
  });

  test("an address that isn't a URL yet", () => {
    expect(isSameHost("", origin)).toBe(false);
    expect(isSameHost("spares.example.com", origin)).toBe(false);
  });
});
