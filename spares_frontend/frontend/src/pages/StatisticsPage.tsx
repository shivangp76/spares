import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { advanceCards, getStatistics, postponeCards } from '../api/client';
import ActionResult, { type ActionOutcome } from '../components/ActionResult';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { STATE_LABELS, type StatisticsResponse } from '../types/spares';
import { formatDuration, td, th } from '../utils';

const sectionTitle: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', margin: '32px 0 12px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const tile: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 6, padding: '12px 20px', textAlign: 'center', minWidth: 96 };

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

function stateLabel(stateId: string): string {
  return STATE_LABELS[Number(stateId)] ?? stateId;
}

function Tile({ value, label }: { value: React.ReactNode; label: string }) {
  return (
    <div className="tile" style={tile}>
      <div style={{ fontSize: 24, fontWeight: 600 }}>{value}</div>
      <div style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{label}</div>
    </div>
  );
}

function StateCounts({ counts, empty }: { counts: Record<string, number>; empty: string }) {
  const entries = Object.entries(counts)
    .filter(([, count]) => count > 0)
    .sort(([a], [b]) => Number(a) - Number(b));
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  if (entries.length === 0) return <p style={{ color: 'var(--text-secondary)' }}>{empty}</p>;
  return (
    <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
      {entries.map(([stateId, count]) => <Tile key={stateId} value={count} label={stateLabel(stateId)} />)}
      <Tile value={total} label="Total" />
    </div>
  );
}

/** `spares card advance` / `spares card postpone`, prefilled with the number of cards that are safe to move. */
function ScheduleForm({ kind, safeCount, onDone }: { kind: 'Advance' | 'Postpone'; safeCount: number; onDone: (outcome: ActionOutcome) => void }) {
  const { credentials } = useAuth();
  const [count, setCount] = useState(String(safeCount));
  const [query, setQuery] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    if (!credentials) return;
    const n = Number(count);
    if (!Number.isInteger(n) || n <= 0) { setError(`Invalid count: ${count}`); return; }
    setRunning(true);
    setError(null);
    try {
      const run = kind === 'Advance' ? advanceCards : postponeCards;
      const eventId = await run(credentials.schedulerName, n, query.trim() || null);
      onDone({ message: `${kind === 'Advance' ? 'Advanced' : 'Postponed'} ${n} cards.`, eventIds: eventId === null ? [] : [eventId] });
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', fontSize: 14, marginTop: 8 }}>
      <input
        type="number"
        min={1}
        value={count}
        onChange={e => setCount(e.target.value)}
        aria-label={`Number of cards to ${kind.toLowerCase()}`}
        style={{ width: 80, padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 }}
      />
      <input
        type="text"
        value={query}
        onChange={e => setQuery(e.target.value)}
        onKeyDown={e => { if (e.key === 'Enter') run(); }}
        placeholder="Optional query, e.g. tag=a"
        style={{ flex: 1, minWidth: 160, padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 }}
      />
      <button onClick={run} disabled={running} className="touch-target-small" style={{ minWidth: 90 }}>{running ? '…' : kind}</button>
      {error && <span style={{ color: 'var(--error)', fontSize: 13 }}>{error}</span>}
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
  // Bumped to refetch after an action changes the schedule
  const [reloadCount, setReloadCount] = useState(0);
  // Numbered so each new outcome gets a fresh ActionResult (and Undo button)
  const [scheduleOutcome, setScheduleOutcome] = useState<{ seq: number; outcome: ActionOutcome } | null>(null);
  const showOutcome = (outcome: ActionOutcome) => setScheduleOutcome(prev => ({ seq: (prev?.seq ?? 0) + 1, outcome }));
  const isToday = date === toDateInputValue(new Date());
  const reload = () => setReloadCount(c => c + 1);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (!date) return;
    let cancelled = false;
    getStatistics(credentials.schedulerName, requestDate(date))
      .then(stats => { if (!cancelled) { setStatistics(stats); setError(null); setLoadedDate(date); } })
      .catch(e => { if (!cancelled) { setError(String(e)); setLoadedDate(date); } });
    return () => { cancelled = true; };
  }, [credentials, navigate, date, reloadCount]);

  const dueByDate = statistics
    ? Object.entries(statistics.due_count_by_date).sort(([a], [b]) => a.localeCompare(b))
    : [];
  const maxDue = Math.max(1, ...dueByDate.map(([, count]) => count));

  return (
    <div className="page">
      <Navbar onLogout={logout} />

      <div style={{ display: 'flex', gap: 12, alignItems: 'center', flexWrap: 'wrap' }}>
        <label style={{ fontSize: 14 }}>
          Date{' '}
          <input
            type="date"
            value={date}
            onChange={e => setDate(e.target.value)}
            style={{ padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 }}
          />
        </label>
        <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>Scheduler: {credentials?.schedulerName}</span>
        {loading && <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading…</span>}
      </div>

      {error && !loading && <div style={{ color: 'var(--error)', marginTop: 16 }}>Error: {error}</div>}

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
          {isToday ? (
            <>
              {/* Keyed on the safe count so the prefilled count follows the latest statistics */}
              <ScheduleForm key={`a${statistics.advance_safe_count}`} kind="Advance" safeCount={statistics.advance_safe_count} onDone={o => { showOutcome(o); reload(); }} />
              <ScheduleForm key={`p${statistics.postpone_safe_count}`} kind="Postpone" safeCount={statistics.postpone_safe_count} onDone={o => { showOutcome(o); reload(); }} />
              {scheduleOutcome && <ActionResult key={scheduleOutcome.seq} outcome={scheduleOutcome.outcome} onUndone={reload} />}
            </>
          ) : (
            <p style={{ fontSize: 13, color: 'var(--text-muted)' }}>Advancing and postponing apply from now, so they are only offered for today.</p>
          )}

          <div style={sectionTitle}>All cards by state</div>
          <StateCounts counts={statistics.card_count_by_state} empty="No cards." />

          <div style={sectionTitle}>Upcoming due</div>
          {dueByDate.length === 0 ? (
            <p style={{ color: 'var(--text-secondary)' }}>No cards due after this date.</p>
          ) : (
            <div className="table-scroll">
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
                        <div style={{ height: 8, borderRadius: 4, background: 'var(--accent)', width: `${(count / maxDue) * 100}%` }} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
