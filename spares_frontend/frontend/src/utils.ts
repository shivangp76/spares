import type { GetReviewCardResponse } from './types/spares';

/** Port of the CLI's `format_duration`, e.g. `7d 0h 0m 0s` or `1m 30s`. */
export function formatDuration(totalSeconds: number): string {
  const days = Math.floor(totalSeconds / 86400);
  const hours = Math.floor((totalSeconds % 86400) / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0 || parts.length) parts.push(`${hours}h`);
  if (minutes > 0 || parts.length) parts.push(`${minutes}m`);
  parts.push(`${seconds}s`);
  return parts.join(' ');
}

export const sectionLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 };
export const th: React.CSSProperties = { textAlign: 'left', padding: '8px 12px', borderBottom: '1px solid var(--border-strong)' };
export const td: React.CSSProperties = { padding: '8px 12px', borderBottom: '1px solid var(--border-subtle)' };

/** The rendered file shown as a card's back: its own back, or the whole note. */
export function backPath(card: GetReviewCardResponse): string {
  const b = card.card_back_rendered_path;
  return 'CardBack' in b ? b.CardBack : b.Note;
}

/** Sets or removes the search `q` in `params`, keeping the page to return to when it is cleared. */
export function withSearch(params: URLSearchParams, q: string | null): URLSearchParams {
  const next = new URLSearchParams(params);
  if (q === null) next.delete('q');
  else next.set('q', q);
  return next;
}
