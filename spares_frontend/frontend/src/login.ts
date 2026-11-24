/**
 * Whether `serverUrl`, as typed on the login page, is the host at `origin`. Signing up and
 * resetting a password talk to the host that served the page, so the login page offers them only
 * for it.
 */
export function isSameHost(serverUrl: string, origin: string): boolean {
  try {
    return new URL(serverUrl.trim()).origin === new URL(origin).origin;
  } catch {
    return false;
  }
}
