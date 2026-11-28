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

/**
 * The name of the note `href` links to, which is its ID, if it is a link to a linked note: those
 * point at the note's raw file, e.g. `/…/abc.md`, in the directory and with the extension of the
 * linked note's parser, which can be another than the linking note's (`.typ`, `.tex`). Not a
 * URL, such as a web page's.
 */
export function linkedNoteName(href: string): string | null {
  // A web page's address can end in `.md` too
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return null;
  return /(?:^|\/)([a-z]{1,8})\.(?:md|typ|tex)$/.exec(href)?.[1] ?? null;
}
