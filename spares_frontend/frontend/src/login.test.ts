import { describe, expect, test } from "bun:test";
import {
  DEFAULT_AFTER_LOGIN,
  hostOf,
  isSameHost,
  loginPath,
  nextPath,
} from "./login";

const place = (pathname: string, search = "", hash = "") => ({
  pathname,
  search,
  hash,
});

describe("loginPath", () => {
  test("returns to the page, with its query and hash", () => {
    expect(loginPath(place("/cards", "?q=tag:a b", "#x"))).toBe(
      `/login?next=${encodeURIComponent("/cards?q=tag:a b#x")}`,
    );
  });

  test("the root and the login pages themselves return nowhere in particular", () => {
    for (const p of ["/", "/login", "/signup", "/reset-password"])
      expect(loginPath(place(p, "?next=/cards"))).toBe("/login");
  });

  test("round-trips through nextPath", () => {
    const path = loginPath(place("/notes", "?q=a&b=c%20d", "#n1"));
    expect(nextPath(path.slice("/login".length))).toBe("/notes?q=a&b=c%20d#n1");
  });
});

describe("nextPath", () => {
  const next = (value: string) =>
    nextPath(`?${new URLSearchParams({ next: value })}`);

  test("a page of the app", () => {
    expect(next("/statistics")).toBe("/statistics");
    expect(next("/manual/concepts#cards")).toBe("/manual/concepts#cards");
  });

  test("none, or a login page, goes to the review page", () => {
    expect(nextPath("")).toBe(DEFAULT_AFTER_LOGIN);
    expect(nextPath("?verified")).toBe(DEFAULT_AFTER_LOGIN);
    expect(next("/login")).toBe(DEFAULT_AFTER_LOGIN);
    expect(next("/signup?invite=x")).toBe(DEFAULT_AFTER_LOGIN);
  });

  test("never another site", () => {
    for (const v of [
      "https://evil.example.com/",
      "//evil.example.com/x",
      "/\\evil.example.com",
      "javascript:alert(1)",
      "cards",
    ])
      expect(next(v)).toBe(DEFAULT_AFTER_LOGIN);
  });
});

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

describe("hostOf", () => {
  test("a URL's host and port", () => {
    expect(hostOf(" https://spares.example.com:8443/x ")).toBe(
      "spares.example.com:8443",
    );
  });

  test("what was typed, if it isn't a URL", () => {
    expect(hostOf("spares.example")).toBe("spares.example");
    expect(hostOf("  ")).toBe("no host");
  });
});
