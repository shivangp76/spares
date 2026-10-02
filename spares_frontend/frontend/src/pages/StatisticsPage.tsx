import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStatistics } from '../api/client';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { STATE_LABELS, type StatisticsResponse } from '../types/spares';

const sectionTitle: React.CSSProperties = { fontSize: 12, color: '#888', margin: '32px 0 12px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const tile: React.CSSProperties = { border: '1px solid #ddd', borderRadius: 6, padding: '12px 20px', textAlign: 'center', minWidth: 96 };
const th: React.CSSProperties = { textAlign: 'left', padding: '8px 12px', borderBottom: '1px solid #ccc' };
const td: React.CSSProperties = { padding: '8px 12px', borderBottom: '1px solid #eee' };

/** `YYYY-MM-DD` for `date` in local time, the format `<input type="date">` uses. */
function toDateInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** The instant to request statistics for: now if `value` is today, otherwise local noon of that day. */
function requestDate(value: string): Date {
  if (value === toDateInputValue(new Date())) return new Date();
  const [year, month, day] = value.split('-').map(Number);
  return new Date(year, month - 1, day, 12);
}

/** Matches the CLI's `format_duration`, e.g. `1h 0m 5s`. */
function formatDuration(totalSeconds: number): string {
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

function stateLabel(stateId: string): string {
  return STATE_LABELS[Number(stateId)] ?? stateId;
}

function Tile({ value, label }: { value: React.ReactNode; label: string }) {
  return (
    <div style={tile}>
      <div style={{ fontSize: 24, fontWeight: 600 }}>{value}</div>
      <div style={{ fontSize: 13, color: '#666' }}>{label}</div>
    </div>
  );
}

function StateCounts({ counts, empty }: { counts: Record<string, number>; empty: string }) {
  const entries = Object.entries(counts)
    .filter(([, count]) => count > 0)
    .sort(([a], [b]) => Number(a) - Number(b));
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  if (entries.length === 0) return <p style={{ color: '#555' }}>{empty}</p>;
  return (
    <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
      {entries.map(([stateId, count]) => <Tile key={stateId} value={count} label={stateLabel(stateId)} />)}
      <Tile value={total} label="Total" />
    </div>
  );
}

export default function StatisticsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();

  const [date, setDate] = useState(() => toDateInputValue(new Date()));
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(null);
  // The date the latest response (statistics or error) is for; loading while it lags `date`
  const [loadedDate, setLoadedDate] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loading = !!date && loadedDate !== date;

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (!date) return;
    let cancelled = false;
    getStatistics(credentials.schedulerName, requestDate(date))
      .then(stats => { if (!cancelled) { setStatistics(stats); setError(null); setLoadedDate(date); } })
      .catch(e => { if (!cancelled) { setError(String(e)); setLoadedDate(date); } });
    return () => { cancelled = true; };
  }, [credentials, navigate, date]);

  const dueByDate = statistics
    ? Object.entries(statistics.due_count_by_date).sort(([a], [b]) => a.localeCompare(b))
    : [];
  const maxDue = Math.max(1, ...dueByDate.map(([, count]) => count));

  return (
    <div style={{ maxWidth: 800, margin: '0 auto', padding: 24 }}>
      <Navbar onLogout={logout} />

      <div style={{ display: 'flex', gap: 12, alignItems: 'center', flexWrap: 'wrap' }}>
        <label style={{ fontSize: 14 }}>
          Date{' '}
          <input
            type="date"
            value={date}
            onChange={e => setDate(e.target.value)}
            style={{ padding: '6px 10px', fontSize: 14, border: '1px solid #ccc', borderRadius: 4 }}
          />
        </label>
        <span style={{ fontSize: 13, color: '#888' }}>Scheduler: {credentials?.schedulerName}</span>
        {loading && <span style={{ fontSize: 13, color: '#888' }}>Loading…</span>}
      </div>

      {error && !loading && <div style={{ color: 'red', marginTop: 16 }}>Error: {error}</div>}

      {statistics && (
        <div style={{ opacity: loading ? 0.5 : 1 }}>
          <div style={sectionTitle}>Studied</div>
          <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
            <Tile value={statistics.cards_studied_count} label="Cards studied" />
            <Tile value={formatDuration(statistics.recall_duration)} label="Recall time" />
            <Tile value={formatDuration(statistics.rate_duration)} label="Rate time" />
            <Tile value={formatDuration(statistics.recall_duration + statistics.rate_duration)} label="Total time" />
          </div>

          <div style={sectionTitle}>Due</div>
          <StateCounts counts={statistics.due_count_by_state} empty="Nothing due." />

          <div style={sectionTitle}>Scheduling</div>
          <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
            <Tile value={statistics.advance_safe_count} label="Safe to advance" />
            <Tile value={statistics.postpone_safe_count} label="Safe to postpone" />
          </div>

          <div style={sectionTitle}>All cards by state</div>
          <StateCounts counts={statistics.card_count_by_state} empty="No cards." />

          <div style={sectionTitle}>Upcoming due</div>
          {dueByDate.length === 0 ? (
            <p style={{ color: '#555' }}>No cards due after this date.</p>
          ) : (
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
              <thead>
                <tr>
                  <th style={th}>Date</th>
                  <th style={{ ...th, textAlign: 'right', width: 80 }}>Cards</th>
                  <th style={th} />
                </tr>
              </thead>
              <tbody>
                {dueByDate.map(([day, count]) => (
                  <tr key={day}>
                    <td style={{ ...td, whiteSpace: 'nowrap' }}>{day}</td>
                    <td style={{ ...td, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>{count}</td>
                    <td style={{ ...td, width: '60%' }}>
                      <div style={{ height: 8, borderRadius: 4, background: '#4a7bd0', width: `${(count / maxDue) * 100}%` }} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
