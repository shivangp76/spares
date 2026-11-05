/**
 * `href`, from a card, resolved against `base`, if it is safe to open in a new tab: a web page, a
 * file of the app's, or an email address. `javascript:` and other schemes would run or show
 * something with the app's origin, and a card can come from someone else's shared notes.
 */
export function openableUrl(href: string, base: string): string | null {
  let url: URL;
  try {
    url = new URL(href, base);
  } catch {
    return null;
  }
  return ["http:", "https:", "mailto:"].includes(url.protocol)
    ? url.href
    : null;
}
