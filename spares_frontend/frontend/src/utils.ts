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

export const sectionLabel: React.CSSProperties = { fontSize: 12, color: '#888', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em', marginBottom: 6 };
export const th: React.CSSProperties = { textAlign: 'left', padding: '8px 12px', borderBottom: '1px solid #ccc' };
export const td: React.CSSProperties = { padding: '8px 12px', borderBottom: '1px solid #eee' };
