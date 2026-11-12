/**
 * Where the navbar shows a page's link: `main` in the row of links, `more` in its More menu,
 * `account` in the account menu, and `undo` in the Undo button's menu.
 */
export type NavPlace = "main" | "more" | "account" | "undo";

/** The pages the navbar links to, in its order, with the key that goes to each after `g`. */
export const NAV_LINKS: {
  to: string;
  label: string;
  key: string;
  place: NavPlace;
}[] = [
  { to: "/review", label: "Review", key: "r", place: "main" },
  { to: "/notes", label: "Notes", key: "n", place: "main" },
  { to: "/cards", label: "Cards", key: "c", place: "main" },
  { to: "/graph", label: "Graph", key: "d", place: "main" },
  { to: "/statistics", label: "Statistics", key: "s", place: "main" },
  { to: "/tags", label: "Tags", key: "t", place: "more" },
  { to: "/parsers", label: "Parsers", key: "p", place: "more" },
  { to: "/keywords", label: "Keywords", key: "k", place: "more" },
  { to: "/image-occlusion", label: "Image Occlusion", key: "i", place: "more" },
  { to: "/manual", label: "Manual", key: "m", place: "more" },
  { to: "/history", label: "History", key: "h", place: "undo" },
  // As Cmd+, opens an app's settings on a Mac
  { to: "/settings", label: "Settings", key: ",", place: "account" },
];

/** Whether `pathname` is the page `to` or one under it, as `/manual/concepts` is under `/manual`. */
export function isOnPage(pathname: string, to: string): boolean {
  return pathname === to || pathname.startsWith(`${to}/`);
}
