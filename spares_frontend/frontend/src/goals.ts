/** Links to review a goal of config.toml (`[goals.<name>]`) or one of its parts. */

/** `minutes` as "2 h 10 min" or "25 min". */
export function formatMinutes(minutes: number): string {
  const total = Math.round(Math.max(0, minutes));
  const hours = Math.floor(total / 60);
  const rest = total % 60;
  if (hours === 0) return `${rest} min`;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

/** The Review page path that starts a review of the cards due today that `query` finds. */
export function reviewQueryPath(query: string): string {
  return `/review?query=${encodeURIComponent(query)}&start`;
}
