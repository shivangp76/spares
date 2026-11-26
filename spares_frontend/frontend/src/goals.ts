/** Links to review a goal of config.toml (`[goals.<name>]`) or one of its parts. */

/** The Review page path that starts a review of the cards due today that `query` finds. */
export function reviewQueryPath(query: string): string {
  return `/review?query=${encodeURIComponent(query)}&start`;
}
