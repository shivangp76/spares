/** The pages before logging in, which are never where logging in returns to. */
const AUTH_PATHS = ["/login", "/signup", "/reset-password"];

/** Where logging in goes when nothing (or nothing safe) says where. */
export const DEFAULT_AFTER_LOGIN = "/review";

type Place = Pick<Location, "pathname" | "search" | "hash">;

/** The login page's path, saying to return to `here` (by default the current page) afterwards. */
export function loginPath(here: Place = window.location): string {
  if (here.pathname === "/" || AUTH_PATHS.includes(here.pathname))
    return "/login";
  const next = here.pathname + here.search + here.hash;
  return `/login?next=${encodeURIComponent(next)}`;
}

/**
 * Where to go after logging in, from the login page's `search`: its `next` when that is a page of
 * this app, else the review page. Anything else (another site, `//host`, a login page) is ignored,
 * so a link can't send someone elsewhere by logging them in.
 */
export function nextPath(search: string): string {
  const next = new URLSearchParams(search).get("next");
  if (!next?.startsWith("/")) return DEFAULT_AFTER_LOGIN;
  const base = "http://app.invalid";
  let url: URL;
  try {
    url = new URL(next, base);
  } catch {
    return DEFAULT_AFTER_LOGIN;
  }
  if (url.origin !== base || AUTH_PATHS.includes(url.pathname))
    return DEFAULT_AFTER_LOGIN;
  return url.pathname + url.search + url.hash;
}
