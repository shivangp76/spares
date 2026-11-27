/** Learning steps as typed in the interval preview of the scheduler settings, e.g. `1m 10m`. */

const UNITS: Record<string, number> = { s: 1, m: 60, h: 3600 };

/** The seconds of steps like `30s 1m 10m 1h`, or null if one isn't a number with a unit. */
export function parseSteps(text: string): number[] | null {
  const steps: number[] = [];
  for (const token of text.trim().split(/[\s,]+/).filter(Boolean)) {
    const match = /^(\d+(?:\.\d+)?)([smh])$/.exec(token);
    if (!match) return null;
    steps.push(Math.round(Number(match[1]) * UNITS[match[2]]));
  }
  return steps;
}

/** Steps in seconds, as `parseSteps` reads them back. */
export function formatSteps(seconds: readonly number[]): string {
  return seconds
    .map((s) => (s % 3600 === 0 ? `${s / 3600}h` : s % 60 === 0 ? `${s / 60}m` : `${s}s`))
    .join(' ');
}
