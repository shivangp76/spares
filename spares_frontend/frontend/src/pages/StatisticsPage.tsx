import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { advanceCards, ApiError, getReviewHistory, getStatistics, getStatisticsGraphs, getStepsStats, postponeCards } from '../api/client';
import { useServerReachable } from '../api/reachability';
import ActionResult, { type ActionOutcome } from '../components/ActionResult';
import BarChart from '../components/charts/BarChart';
import { cumulative, daysBetween, fillDays, HOUR_LABELS, percent, toSeries, type Series } from '../components/charts/chartData';
import Histogram from '../components/charts/Histogram';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import ReviewHeatmap from '../components/ReviewHeatmap';
import { useAuth } from '../hooks/useAuth';
import { useCardStates } from '../hooks/useCardStates';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useLocalStatus } from '../local/copy';
import {
  type AnswerGroup,
  type PassFail,
  type ReviewHistoryResponse,
  type StatisticsGraphsResponse,
  type StatisticsResponse,
  type StepsGroupKind,
  type StepsStatsResponse,
} from '../types/spares';
import { errorMessage, formatDuration, td, th } from '../utils';

const sectionTitle: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', margin: '32px 0 12px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const tile: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 6, padding: '12px 20px', textAlign: 'center', minWidth: 96 };
const tiles: React.CSSProperties = { display: 'flex', gap: 16, flexWrap: 'wrap' };
const note: React.CSSProperties = { fontSize: 13, color: 'var(--text-muted)', margin: '8px 0 0' };
const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };
const chip: React.CSSProperties = { padding: '4px 10px', fontSize: 13 };
const numeric: React.CSSProperties = { ...td, textAlign: 'right', fontVariantNumeric: 'tabular-nums' };
const OFFLINE_HINT = 'The server cannot be reached, and advancing and postponing need it.';

/** What the page was asked to show, apart from the date. */
interface StatisticsView {
  query: string;
  /** `null` for all time. */
  periodDays: number | null;
  forecastDays: number;
}

const PERIODS: { label: string; days: number | null }[] = [
  { label: '1 month', days: 30 },
  { label: '3 months', days: 90 },
  { label: '1 year', days: 365 },
  { label: 'All', days: null },
];
const FORECAST_DAYS = [7, 30, 90, 365];

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

function Tile({ value, label }: { value: React.ReactNode; label: string }) {
  return (
    <div className="tile" style={tile}>
      <div style={{ fontSize: 24, fontWeight: 600 }}>{value}</div>
      <div style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{label}</div>
    </div>
  );
}

function StateCounts({ counts, empty }: { counts: Record<string, number>; empty: string }) {
  const { stateName } = useCardStates();
  const entries = Object.entries(counts)
    .filter(([, count]) => count > 0)
    .sort(([a], [b]) => Number(a) - Number(b));
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  if (entries.length === 0) return <p style={{ color: 'var(--text-secondary)' }}>{empty}</p>;
  return (
    <div style={tiles}>
      {entries.map(([stateId, count]) => <Tile key={stateId} value={count} label={stateName(Number(stateId))} />)}
      <Tile value={total} label="Total" />
    </div>
  );
}

/** `spares card advance` / `spares card postpone`, prefilled with the number of cards that are safe to move. */
function ScheduleForm({ kind, safeCount, disabled, onDone }: { kind: 'Advance' | 'Postpone'; safeCount: number; disabled: boolean; onDone: (outcome: ActionOutcome) => void }) {
  const { credentials } = useAuth();
  const [count, setCount] = useState(String(safeCount));
  const [query, setQuery] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    if (!credentials || disabled) return;
    const n = Number(count);
    if (!Number.isInteger(n) || n <= 0) { setError(`Invalid count: ${count}`); return; }
    setRunning(true);
    setError(null);
    try {
      const run = kind === 'Advance' ? advanceCards : postponeCards;
      const eventId = await run(credentials.schedulerName, n, query.trim() || null);
      onDone({ message: `${kind === 'Advance' ? 'Advanced' : 'Postponed'} ${n} cards.`, eventIds: eventId === null ? [] : [eventId] });
    } catch (e) {
      setError(errorMessage(e));
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
        disabled={disabled}
        style={{ ...input, width: 80 }}
      />
      <input
        type="text"
        value={query}
        onChange={e => setQuery(e.target.value)}
        onKeyDown={e => { if (e.key === 'Enter') run(); }}
        placeholder="Optional query, e.g. tag=a"
        disabled={disabled}
        style={{ ...input, flex: 1, minWidth: 160 }}
      />
      <button onClick={run} disabled={running || disabled} title={disabled ? OFFLINE_HINT : undefined} className="touch-target-small" style={{ minWidth: 90 }}>{running ? '…' : kind}</button>
      {error && <span style={{ color: 'var(--error)', fontSize: 13, whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>{error}</span>}
    </div>
  );
}

function TodaySection({ statistics, graphs, title }: { statistics: StatisticsResponse | null; graphs: StatisticsGraphsResponse | null; title: string }) {
  const today = graphs?.today;
  return (
    <>
      <div style={sectionTitle}>{title}</div>
      <div style={tiles}>
        {statistics && (
          <>
            <Tile value={statistics.cards_studied_count} label="Cards studied" />
            <Tile value={formatDuration(statistics.recall_duration)} label="Recall time" />
            <Tile value={formatDuration(statistics.rate_duration)} label="Rate time" />
            <Tile value={formatDuration(statistics.recall_duration + statistics.rate_duration)} label="Total time" />
          </>
        )}
        {today && today.reviews > 0 && (
          <>
            <Tile value={percent(today.passed, today.reviews)} label="Correct" />
            <Tile value={formatDuration(Math.round(today.seconds / today.reviews))} label="Per card" />
            <Tile value={today.learning + today.relearning} label="Learning" />
            <Tile value={today.young + today.mature} label="Review" />
          </>
        )}
        {/* Ratings in filtered tags that do not reschedule, apart from the reviews; only on days with some */}
        {today && today.crams > 0 && (
          <>
            <Tile value={today.crams} label="Crams" />
            <Tile value={formatDuration(today.cram_seconds)} label="Cram time" />
            <Tile value={percent(today.crams_passed, today.crams)} label="Cram correct" />
          </>
        )}
      </div>
      {today && today.reviews > 0 && (
        <p style={note}>
          {today.reviews} reviews: {today.learning} learning, {today.relearning} relearning, {today.young} young, {today.mature} mature. Crams count apart, with their own % correct.
        </p>
      )}
    </>
  );
}

const FORECAST_SPECS = [
  { key: 'learning', label: 'Learning', color: 'var(--chart-learning)' },
  { key: 'review', label: 'Review', color: 'var(--chart-mature)' },
];

function ForecastSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const { labels, series, total } = useMemo(() => {
    const series = toSeries(graphs.forecast.map(day => [day.learning, day.review]), FORECAST_SPECS);
    return { labels: graphs.forecast.map(day => day.date), series, total: graphs.forecast.reduce((sum, day) => sum + day.learning + day.review, 0) };
  }, [graphs]);
  const line = useMemo(() => ({
    label: 'Cumulative',
    color: 'var(--chart-line)',
    values: cumulative(graphs.forecast.map(day => day.learning + day.review)),
    merge: 'last' as const,
  }), [graphs]);
  if (total === 0) return <p style={{ color: 'var(--text-secondary)' }}>Nothing due.</p>;
  const today = graphs.forecast[0];
  return (
    <>
      <BarChart title="Cards due per day" labels={labels} series={series} line={line} />
      <p style={note}>
        {total.toLocaleString()} due in the next {graphs.forecast.length} days, {((today?.learning ?? 0) + (today?.review ?? 0)).toLocaleString()} of them today (overdue included). New and suspended cards are left out.
      </p>
    </>
  );
}

const REVIEW_SPECS = [
  { key: 'learning', label: 'Learning', color: 'var(--chart-learning)' },
  { key: 'relearning', label: 'Relearning', color: 'var(--chart-relearning)' },
  { key: 'young', label: 'Young', color: 'var(--chart-young)' },
  { key: 'mature', label: 'Mature', color: 'var(--chart-mature)' },
  { key: 'cram', label: 'Cram', color: 'var(--chart-cram)' },
];

function ReviewsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const [showTime, setShowTime] = useState(false);
  const days = useMemo(() => daysBetween(graphs.period_start, graphs.date), [graphs]);
  const counts = useMemo(
    () => toSeries(fillDays(days, graphs.reviews, d => [d.learning, d.relearning, d.young, d.mature, d.cram], 5), REVIEW_SPECS),
    [days, graphs],
  );
  const minutes: Series[] = useMemo(
    () => [{ key: 'minutes', label: 'Minutes', color: 'var(--chart-bar)', values: fillDays(days, graphs.reviews, d => [d.seconds / 60], 1).map(([m]) => m) }],
    [days, graphs],
  );
  const summary = graphs.reviews_summary;
  if (summary.reviews + summary.crams === 0) return <p style={{ color: 'var(--text-secondary)' }}>No reviews in this period.</p>;
  return (
    <>
      <div style={{ display: 'flex', gap: 4, marginBottom: 8 }}>
        <button style={chip} aria-pressed={!showTime} onClick={() => setShowTime(false)}>Count</button>
        <button style={chip} aria-pressed={showTime} onClick={() => setShowTime(true)}>Time</button>
      </div>
      {showTime
        ? <BarChart title="Minutes studied per day" labels={days} series={minutes} />
        : <BarChart title="Reviews per day" labels={days} series={counts} />}
      <div style={{ ...tiles, marginTop: 12 }}>
        <Tile value={summary.reviews.toLocaleString()} label="Reviews" />
        {summary.crams > 0 && <Tile value={summary.crams.toLocaleString()} label="Crams" />}
        <Tile value={summary.daily_average.toFixed(1)} label="Daily average" />
        <Tile value={`${summary.days_studied}/${summary.days}`} label="Days studied" />
        <Tile value={formatDuration(summary.seconds)} label="Time" />
      </div>
    </>
  );
}

function rate({ passed, failed }: PassFail): string {
  return percent(passed, passed + failed);
}

function RetentionSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const groups = (row: { young: PassFail; mature: PassFail }) => [
    row.young,
    row.mature,
    { passed: row.young.passed + row.mature.passed, failed: row.young.failed + row.mature.failed },
  ];
  return (
    <>
      <div className="table-scroll">
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
          <thead>
            <tr>
              <th style={th} rowSpan={2} />
              <th style={{ ...th, textAlign: 'center' }} colSpan={3}>Young</th>
              <th style={{ ...th, textAlign: 'center' }} colSpan={3}>Mature (≥ {graphs.mature_threshold_days} days)</th>
              <th style={{ ...th, textAlign: 'center' }} colSpan={3}>Total</th>
            </tr>
            <tr>
              {[0, 1, 2].flatMap(g => ['Pass', 'Fail', 'Retention'].map(h => <th key={`${g}${h}`} style={{ ...th, textAlign: 'right', fontWeight: 500 }}>{h}</th>))}
            </tr>
          </thead>
          <tbody>
            {graphs.true_retention.map(row => (
              <tr key={row.label}>
                <td style={{ ...td, whiteSpace: 'nowrap' }}>{row.label}</td>
                {groups(row).flatMap((g, i) => [
                  <td key={`${i}p`} style={numeric}>{g.passed.toLocaleString()}</td>,
                  <td key={`${i}f`} style={numeric}>{g.failed.toLocaleString()}</td>,
                  <td key={`${i}r`} style={{ ...numeric, fontWeight: 600 }}>{rate(g)}</td>,
                ])}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p style={note}>Reviews of cards in the review state that were passed. Learning steps and crams are left out.</p>
    </>
  );
}

function CardCountsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const { stateName, stateRole } = useCardStates();
  const counts = graphs.card_counts;
  if (counts.total === 0) return <p style={{ color: 'var(--text-secondary)' }}>No cards.</p>;
  const states = Object.entries(counts.by_state).filter(([, n]) => n > 0).sort(([a], [b]) => Number(a) - Number(b));
  // Young and mature count the cards of every review state, in place of them
  const firstReview = states.find(([state]) => stateRole(Number(state)) === 'Review')?.[0];
  return (
    <div style={tiles}>
      {states.flatMap(([state, n]) => stateRole(Number(state)) !== 'Review'
        ? [<Tile key={state} value={n} label={stateName(Number(state))} />]
        : state === firstReview
          ? [<Tile key="young" value={counts.young} label="Young" />, <Tile key="mature" value={counts.mature} label={`Mature (≥ ${graphs.mature_threshold_days}d)`} />]
          : [])}
      <Tile value={counts.suspended} label="Suspended" />
      <Tile value={counts.buried} label="Buried" />
      <Tile value={counts.total} label="Total" />
    </div>
  );
}

function AnswerButtonsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const { ratings, learning, young, mature } = graphs.answer_buttons;
  const rows: [string, AnswerGroup][] = [['Learning', learning], ['Young', young], ['Mature', mature]];
  return (
    <div className="table-scroll">
      <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
        <thead>
          <tr>
            <th style={th} />
            {ratings.map(r => <th key={r.id} style={{ ...th, textAlign: 'right' }}>{r.description}</th>)}
            <th style={{ ...th, textAlign: 'right' }}>Correct</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(([label, group]) => (
            <tr key={label}>
              <td style={td}>{label}</td>
              {group.counts.map(([id, count]) => (
                <td key={id} style={numeric}>
                  {count.toLocaleString()} <span style={{ color: 'var(--text-muted)' }}>({percent(count, group.total)})</span>
                  <div aria-hidden style={{ height: 4, borderRadius: 2, background: 'var(--chart-bar)', width: `${group.total ? (count / group.total) * 100 : 0}%`, marginLeft: 'auto', marginTop: 2 }} />
                </td>
              ))}
              <td style={{ ...numeric, fontWeight: 600 }}>{percent(group.passed, group.total)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function HourlySection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const [showCrams, setShowCrams] = useState(false);
  const hasCrams = graphs.hourly.some(h => h.crams > 0);
  const series: Series[] = useMemo(() => [
    { key: 'reviews', label: 'Reviews', color: 'var(--chart-bar)', values: graphs.hourly.map(h => h.reviews) },
    ...(showCrams ? [{ key: 'crams', label: 'Crams', color: 'var(--chart-cram)', values: graphs.hourly.map(h => h.crams) }] : []),
  ], [graphs, showCrams]);
  const line = useMemo(() => ({
    label: showCrams ? '% correct (reviews and crams apart)' : '% correct',
    color: 'var(--chart-line)',
    values: graphs.hourly.map(h => (h.reviews ? (h.passed / h.reviews) * 100 : null)),
    format: (v: number) => `${Math.round(v)}%`,
    max: 100,
  }), [graphs, showCrams]);
  if (graphs.hourly.every(h => h.reviews + h.crams === 0)) return <p style={{ color: 'var(--text-secondary)' }}>No reviews in this period.</p>;
  return (
    <>
      {hasCrams && (
        <label style={{ fontSize: 13, display: 'inline-flex', gap: 6, alignItems: 'center', marginBottom: 8 }}>
          <input type="checkbox" checked={showCrams} onChange={e => setShowCrams(e.target.checked)} />
          Show crams
        </label>
      )}
      <BarChart title="Reviews by hour" labels={HOUR_LABELS} series={series} line={line} labelHeading="Hour" mergeToFit={false} />
      {showCrams && (
        <p style={note}>
          Crams correct: {graphs.hourly.map((h, i) => (h.crams ? `${HOUR_LABELS[i]}h ${percent(h.crams_passed, h.crams)}` : null)).filter(Boolean).join(', ')}
        </p>
      )}
    </>
  );
}

function AddedSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const days = useMemo(() => daysBetween(graphs.period_start, graphs.date), [graphs]);
  const series: Series[] = useMemo(
    () => [{ key: 'added', label: 'Cards', color: 'var(--chart-bar)', values: fillDays(days, graphs.added, d => [d.cards], 1).map(([n]) => n) }],
    [days, graphs],
  );
  const total = graphs.added.reduce((sum, d) => sum + d.cards, 0);
  if (total === 0) return <p style={{ color: 'var(--text-secondary)' }}>No cards added in this period.</p>;
  return (
    <>
      <BarChart title="Cards added per day" labels={days} series={series} height={100} />
      <p style={note}>{total.toLocaleString()} cards, {(total / Math.max(1, days.length)).toFixed(1)} a day.</p>
    </>
  );
}

const formatPercent = (v: number) => `${Math.round(v * 100)}%`;
const formatAverage = (v: number) => (v >= 100 ? Math.round(v).toLocaleString() : v.toFixed(2));

function DistributionsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const d = graphs.distributions;
  const average = (mean: number | null, unit = '') => (mean === null ? null : <p style={note}>Average {formatAverage(mean)}{unit}</p>);
  return (
    <>
      <h3 style={{ fontSize: 14, margin: '8px 0' }}>Interval</h3>
      <Histogram title="Cards by interval" data={d.interval} unit=" days" />
      {average(d.interval.mean, ' days')}
      <h3 style={{ fontSize: 14, margin: '20px 0 8px' }}>Stability</h3>
      <Histogram title="Cards by stability" data={d.stability} unit=" days" />
      {average(d.stability.mean, ' days')}
      <h3 style={{ fontSize: 14, margin: '20px 0 8px' }}>Difficulty</h3>
      <Histogram title="Cards by difficulty" data={d.difficulty} />
      {average(d.difficulty.mean)}
      {d.retrievability && (
        <>
          <h3 style={{ fontSize: 14, margin: '20px 0 8px' }}>Retrievability</h3>
          <Histogram title="Cards by retrievability" data={d.retrievability} format={formatPercent} />
          <div style={{ ...tiles, marginTop: 12 }}>
            {d.retrievability.mean !== null && <Tile value={formatPercent(d.retrievability.mean)} label="Average retrievability" />}
            {d.total_knowledge !== null && <Tile value={Math.round(d.total_knowledge).toLocaleString()} label="Estimated total knowledge" />}
          </div>
          <p style={note}>Total knowledge is the sum of every card’s retrievability: about how many of them you would recall now.</p>
        </>
      )}
      <h3 style={{ fontSize: 14, margin: '20px 0 8px' }}>Cards in review</h3>
      <div style={tiles}>
        <Tile value={formatAverage(d.daily_load)} label="Daily load (reviews a day)" />
        {d.average_retrievability !== null && <Tile value={formatPercent(d.average_retrievability)} label="Average retrievability" />}
        {d.average_stability !== null && <Tile value={`${formatAverage(d.average_stability)} days`} label="Average stability" />}
      </div>
      <p style={note}>
        Over the cards in review, leaving out learning and suspended cards. The daily load is about how many reviews a day they will cost once the collection settles: the sum of 1 / interval in days, an interval under a day counting
        as one.
      </p>
    </>
  );
}

const STEPS_GROUP_LABELS: Record<StepsGroupKind, string> = {
  again: 'Again',
  hard: 'Hard',
  good: 'Good',
  again_then_good: 'Again, then Good',
  relearning: 'Lapse (Again)',
};

/** A step at a useful precision: `45s`, `12m`, `2.5h` or `1.3d`. */
function formatStep(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `${(seconds / 3600).toFixed(1)}h`;
  return `${(seconds / 86400).toFixed(1)}d`;
}

const formatSteps = (steps: (number | null)[]) => (steps.length === 0 ? 'none' : steps.map(step => (step === null ? '?' : formatStep(step))).join(' '));

/** Steps stats for one preset's cards, fetched on their own since they need no query or date. */
function StepsSection({ periodDays, enabled }: { periodDays: number | null; enabled: boolean }) {
  // '' for the cards that use no preset
  const [preset, setPreset] = useState('');
  const [stats, setStats] = useState<StepsStatsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    getStepsStats({ preset: preset || null, period_days: periodDays })
      .then(s => { if (!cancelled) { setStats(s); setError(null); } })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); });
    return () => { cancelled = true; };
  }, [preset, periodDays, enabled]);

  if (!enabled && !stats) return <p style={note}>The server cannot be reached, and the steps need it.</p>;
  const retention = stats ? formatPercent(stats.desired_retention) : '';
  const r = stats?.recommended;
  return (
    <>
      <label style={{ fontSize: 14 }}>
        Preset{' '}
        <select value={preset} onChange={e => setPreset(e.target.value)} style={input} aria-label="Preset">
          <option value="">No preset</option>
          {stats?.presets.map(name => <option key={name} value={name}>{name}</option>)}
        </select>
      </label>
      {error && <div style={{ color: 'var(--error)', marginTop: 8 }}>{error}</div>}
      {stats && r && (
        <>
          <div style={{ overflowX: 'auto', marginTop: 12 }}>
            <table style={{ borderCollapse: 'collapse', fontSize: 14 }}>
              <thead>
                <tr>
                  <th style={th}>After</th>
                  <th style={th}>Reviews</th>
                  <th style={th}>Recalled</th>
                  <th style={th}>Stability</th>
                  <th style={th}>Falls to {retention} after</th>
                </tr>
              </thead>
              <tbody>
                {stats.groups.map(g => (
                  <tr key={g.kind}>
                    <td style={td}>{STEPS_GROUP_LABELS[g.kind]}</td>
                    <td style={numeric}>{g.reviews}{g.reviews > 0 && g.reviews < stats.min_reviews ? ' (too few)' : ''}</td>
                    <td style={numeric}>{g.reviews > 0 ? formatPercent(g.recalled / g.reviews) : '–'}</td>
                    <td style={numeric}>{g.stability === null ? '–' : formatStep(g.stability * 86400)}</td>
                    <td style={numeric}>{g.interval === null ? '–' : formatStep(g.interval)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <table style={{ borderCollapse: 'collapse', fontSize: 14, marginTop: 12 }}>
            <thead>
              <tr>
                <th style={th}></th>
                <th style={th}>Now</th>
                <th style={th}>Recommended</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td style={td}>Learning steps</td>
                <td style={td}>{formatSteps(stats.learning_steps)}</td>
                <td style={td}>{formatSteps([r.first_learning_step, r.second_learning_step])}</td>
              </tr>
              <tr>
                <td style={td}>Relearning steps</td>
                <td style={td}>{formatSteps(stats.relearning_steps)}</td>
                <td style={td}>{formatSteps([r.relearning_step])}</td>
              </tr>
            </tbody>
          </table>
          <p style={note}>
            Each review in a step is paired with the card’s next review within a day, and each group is fitted the stability that best predicts which were recalled. A step is the time recall takes to fall to {retention}. ? means fewer than{' '}
            {stats.min_reviews} reviews in its groups; a step of a day or more can go. Not affected by the search query.
          </p>
        </>
      )}
    </>
  );
}

export default function StatisticsPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const online = useServerReachable();
  const local = useLocalStatus();
  const { recent, save: saveRecent, remove: removeRecent } = useRecentSearches('spares_statistics_recent_searches');

  const [date, setDate] = useState(() => toDateInputValue(new Date()));
  const [queryInput, setQueryInput] = useState('');
  // A query that can't be searched, marked in the box
  const [queryError, setQueryError] = useState<SearchInputError | null>(null);
  const [view, setView] = useState<StatisticsView>({ query: '', periodDays: 30, forecastDays: 30 });
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(null);
  const [graphs, setGraphs] = useState<StatisticsGraphsResponse | null>(null);
  const [history, setHistory] = useState<ReviewHistoryResponse | null>(null);
  // The view and date the latest responses (or errors) are for; loading while it lags
  const requestKey = JSON.stringify([view, date]);
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loading = loadedKey !== requestKey;
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
    const key = JSON.stringify([view, date]);
    const at = requestDate(date);
    const query = view.query.trim() || null;
    // Fetched apart, so the Today tiles do not wait for the heavier graphs
    const statisticsFetch = getStatistics(credentials.schedulerName, at);
    const graphsFetch = getStatisticsGraphs(credentials.schedulerName, { query, periodDays: view.periodDays, forecastDays: view.forecastDays, date: at });
    const historyFetch = getReviewHistory(query);
    statisticsFetch.then(s => { if (!cancelled) setStatistics(s); }).catch(() => {});
    graphsFetch.then(g => { if (!cancelled) setGraphs(g); }).catch(() => {});
    historyFetch.then(h => { if (!cancelled) setHistory(h); }).catch(() => {});
    Promise.allSettled([statisticsFetch, graphsFetch, historyFetch]).then(([s, g, h]) => {
      if (cancelled) return;
      const failed = [g, h].find(r => r.status === 'rejected') as PromiseRejectedResult | undefined;
      setQueryError(query && failed?.reason instanceof ApiError && failed.reason.span ? { query, span: failed.reason.span } : null);
      if (s.status === 'fulfilled' && g.status === 'fulfilled') {
        setError(null);
      } else {
        const failure = [s, g, h].find(r => r.status === 'rejected') as PromiseRejectedResult;
        setError(errorMessage(failure.reason));
        setStatistics(null);
        setGraphs(null);
      }
      setLoadedKey(key);
    });
    return () => { cancelled = true; };
  }, [credentials, navigate, date, view, reloadCount]);

  function applyQuery(query = queryInput) {
    const q = query.trim();
    setQueryInput(q);
    if (q) saveRecent(q);
    setView(v => ({ ...v, query: q }));
  }

  const todayTitle = isToday ? 'Today' : `Studied on ${date}`;
  // A device's copy schedules cards itself, offline too
  const canSchedule = local.active || online;

  return (
    <div className="page">
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
        <SearchInput
          value={queryInput}
          onChange={setQueryInput}
          onSubmit={applyQuery}
          placeholder="Search query, e.g. tag=languages"
          ariaLabel="Only the cards matching this query"
          error={queryError}
          style={{ minWidth: 180, maxWidth: 'none' }}
          inputStyle={input}
        />
        <button onClick={() => applyQuery()} className="touch-target-small">Apply</button>
        {view.query && <button onClick={() => applyQuery('')} className="touch-target-small" title="Every card">Clear</button>}
      </div>
      <RecentSearches recent={recent} onSelect={applyQuery} onRemove={removeRecent} style={{ marginTop: 8 }} />

      <div style={{ display: 'flex', gap: 16, alignItems: 'center', flexWrap: 'wrap', marginTop: 8, fontSize: 14 }}>
        <div role="group" aria-label="Period" style={{ display: 'flex', gap: 4 }}>
          {PERIODS.map(p => (
            <button key={p.label} style={chip} aria-pressed={view.periodDays === p.days} onClick={() => setView(v => ({ ...v, periodDays: p.days }))}>{p.label}</button>
          ))}
        </div>
        <label>
          Forecast{' '}
          <select value={view.forecastDays} onChange={e => setView(v => ({ ...v, forecastDays: Number(e.target.value) }))} style={input}>
            {FORECAST_DAYS.map(d => <option key={d} value={d}>{d} days</option>)}
          </select>
        </label>
        <label>
          Date{' '}
          <input type="date" value={date} onChange={e => setDate(e.target.value)} style={input} />
        </label>
        {loading && <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading…</span>}
      </div>

      {error && !loading && <div style={{ color: 'var(--error)', marginTop: 16, whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>{error}</div>}

      {(statistics || graphs) && (
        <div style={{ opacity: loading ? 0.5 : 1 }}>
          <TodaySection statistics={statistics} graphs={graphs} title={todayTitle} />

          {statistics && (
            <>
              <div style={sectionTitle}>Due</div>
              <StateCounts counts={statistics.due_count_by_state} empty="Nothing due." />
            </>
          )}

          {graphs && (
            <>
              <div style={sectionTitle}>Forecast</div>
              <ForecastSection graphs={graphs} />

              <div style={sectionTitle}>Reviews</div>
              <ReviewsSection graphs={graphs} />

              <div style={sectionTitle}>True retention</div>
              <RetentionSection graphs={graphs} />
            </>
          )}

          {history && (
            <>
              <div style={sectionTitle}>Calendar</div>
              <ReviewHeatmap counts={history.review_count_by_date} cramCounts={history.cram_count_by_date} />
            </>
          )}

          {graphs && (
            <>
              <div style={sectionTitle}>Card counts</div>
              <CardCountsSection graphs={graphs} />

              <div style={sectionTitle}>Answer buttons</div>
              <AnswerButtonsSection graphs={graphs} />

              <div style={sectionTitle}>Hourly breakdown</div>
              <HourlySection graphs={graphs} />

              <div style={sectionTitle}>Added</div>
              <AddedSection graphs={graphs} />

              <div style={sectionTitle}>Distributions</div>
              <DistributionsSection graphs={graphs} />
            </>
          )}

          <div style={sectionTitle}>Learning steps</div>
          <StepsSection periodDays={view.periodDays} enabled={canSchedule} />

          {statistics && (
            <>
              <div style={sectionTitle}>Scheduling</div>
              <div style={tiles}>
                <Tile value={statistics.advance_safe_count} label="Safe to advance" />
                <Tile value={statistics.postpone_safe_count} label="Safe to postpone" />
              </div>
              {isToday ? (
                <>
                  {/* Keyed on the safe count so the prefilled count follows the latest statistics */}
                  <ScheduleForm key={`a${statistics.advance_safe_count}`} kind="Advance" safeCount={statistics.advance_safe_count} disabled={!canSchedule} onDone={o => { showOutcome(o); reload(); }} />
                  <ScheduleForm key={`p${statistics.postpone_safe_count}`} kind="Postpone" safeCount={statistics.postpone_safe_count} disabled={!canSchedule} onDone={o => { showOutcome(o); reload(); }} />
                  {!canSchedule && <p style={note}>{OFFLINE_HINT}</p>}
                  {scheduleOutcome && <ActionResult key={scheduleOutcome.seq} outcome={scheduleOutcome.outcome} onReverted={reload} />}
                </>
              ) : (
                <p style={note}>Advancing and postponing apply from now, so they are only offered for today.</p>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
