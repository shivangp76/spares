/** Retention per tag on the Tags page (`spares_core::api::statistics::tag_retention`). */
import type { TagRetention } from './types/spares';

export type RetentionTone = 'green' | 'orange' | 'red';

/** How a tag's retention in a week is coloured: green from 85%, orange from 70%, red below. */
export function retentionTone(week: number): RetentionTone {
  if (week >= 0.85) return 'green';
  if (week >= 0.7) return 'orange';
  return 'red';
}

/** `tags` sorted weakest first, by retention in a week; tags without reviewed cards last, by name. */
export function weakestFirst<T extends { name: string }>(tags: readonly T[], retention: ReadonlyMap<string, TagRetention>): T[] {
  return [...tags].sort((a, b) => {
    const ra = retention.get(a.name);
    const rb = retention.get(b.name);
    if (ra && rb) return ra.week - rb.week || a.name.localeCompare(b.name);
    if (ra) return -1;
    if (rb) return 1;
    return a.name.localeCompare(b.name);
  });
}

const percent = (value: number) => `${Math.round(value * 100)}%`;

/** The tooltip: both figures and how many cards they are of. */
export function retentionTitle(r: TagRetention): string {
  return `Predicted recall of its ${r.card_count} reviewed card${r.card_count === 1 ? '' : 's'}, those of the tags under it included: ${percent(r.today)} today, ${percent(r.week)} in 7 days without reviewing`;
}

export { percent as formatRetention };
