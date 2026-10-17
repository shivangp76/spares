import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { advanceCards, getReviewHistory, getStatistics, getStatisticsGraphs, isNetworkError, postponeCards } from '../api/client';
import ActionResult, { type ActionOutcome } from '../components/ActionResult';
import BarChart from '../components/charts/BarChart';
import { cumulative, daysBetween, fillDays, HOUR_LABELS, percent, toSeries, type Series } from '../components/charts/chartData';
import Histogram from '../components/charts/Histogram';
import Navbar from '../components/Navbar';
import RecentSearches from '../components/RecentSearches';
import ReviewHeatmap from '../components/ReviewHeatmap';
import { useAuth } from '../hooks/useAuth';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useOfflineStatus } from '../offline/review';
import {
  cachedViews,
  describeView,
  indexedDbStatisticsCache,
  offlineBanner,
  recallStatistics,
  rememberStatistics,
  statisticsKey,
  type CachedStatistics,
  type StatisticsView,
} from '../offline/statistics';
import {
  STATE_LABELS,
  type AnswerGroup,
  type PassFail,
  type ReviewHistoryResponse,
  type StatisticsGraphsResponse,
  type StatisticsResponse,
} from '../types/spares';
import { errorMessage, formatDuration, td, th } from '../utils';

const sectionTitle: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', margin: '32px 0 12px', fontWeight: 600, textTransform: 'uppercase', letterSpacing: '0.05em' };
const tile: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 6, padding: '12px 20px', textAlign: 'center', minWidth: 96 };
const tiles: React.CSSProperties = { display: 'flex', gap: 16, flexWrap: 'wrap' };
const note: React.CSSProperties = { fontSize: 13, color: 'var(--text-muted)', margin: '8px 0 0' };
const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };
const chip: React.CSSProperties = { padding: '4px 10px', fontSize: 13 };
const numeric: React.CSSProperties = { ...td, textAlign: 'right', fontVariantNumeric: 'tabular-nums' };
// The server's message when offline review is used, as `OfflineIndicator` shows it
const OFFLINE_HINT = 'The server cannot be reached. Downloaded cards can still be reviewed, and ratings are sent once it can.';

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
    <div style={tiles}>
      {entries.map(([stateId, count]) => <Tile key={stateId} value={count} label={stateLabel(stateId)} />)}
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
  const counts = graphs.card_counts;
  if (counts.total === 0) return <p style={{ color: 'var(--text-secondary)' }}>No cards.</p>;
  const states = Object.entries(counts.by_state).filter(([, n]) => n > 0).sort(([a], [b]) => Number(a) - Number(b));
  return (
    <div style={tiles}>
      {states.flatMap(([state, n]) => stateLabel(state) === 'Review'
        ? [<Tile key="young" value={counts.young} label="Young" />, <Tile key="mature" value={counts.mature} label={`Mature (≥ ${graphs.mature_threshold_days}d)`} />]
        : [<Tile key={state} value={n} label={stateLabel(state)} />])}
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

function DistributionsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const d = graphs.distributions;
  const average = (mean: number | null, unit = '') => (mean === null ? null : <p style={note}>Average {mean >= 100 ? Math.round(mean).toLocaleString() : mean.toFixed(2)}{unit}</p>);
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
    </>
  );
}

export default function StatisticsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const { online, pending } = useOfflineStatus();
  const { recent, save: saveRecent, remove: removeRecent } = useRecentSearches('spares_statistics_recent_searches');

  const [date, setDate] = useState(() => toDateInputValue(new Date()));
  const [queryInput, setQueryInput] = useState('');
  const [view, setView] = useState<StatisticsView>({ query: '', periodDays: 30, forecastDays: 30 });
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(null);
  const [graphs, setGraphs] = useState<StatisticsGraphsResponse | null>(null);
  const [history, setHistory] = useState<ReviewHistoryResponse | null>(null);
  // The view and date the latest responses (or errors) are for; loading while it lags
  const requestKey = JSON.stringify([view, date]);
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Shown in place of the server's responses when it cannot be reached
  const [offlineEntry, setOfflineEntry] = useState<CachedStatistics | null>(null);
  // The views kept offline, offered when the current one is not among them
  const [otherViews, setOtherViews] = useState<CachedStatistics[] | null>(null);
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
    Promise.allSettled([statisticsFetch, graphsFetch, historyFetch]).then(async ([s, g, h]) => {
      if (cancelled) return;
      const cacheKey = statisticsKey(credentials.serverUrl, view);
      if (s.status === 'fulfilled' && g.status === 'fulfilled') {
        setError(null);
        setOfflineEntry(null);
        setOtherViews(null);
        // Only today's statistics are kept, so a look at a past day does not replace them
        if (isToday) {
          const now = Date.now();
          rememberStatistics(indexedDbStatisticsCache, {
            key: cacheKey,
            serverUrl: credentials.serverUrl,
            view,
            statistics: s.value,
            graphs: g.value,
            history: h.status === 'fulfilled' ? h.value : null,
            fetchedAt: now,
            usedAt: now,
          }).catch(console.error);
        }
      } else {
        const failure = [s, g, h].find(r => r.status === 'rejected') as PromiseRejectedResult;
        const cached = isNetworkError(failure.reason) || !navigator.onLine
          ? await recallStatistics(indexedDbStatisticsCache, cacheKey).catch(() => undefined)
          : undefined;
        if (cancelled) return;
        if (cached) {
          setStatistics(cached.statistics);
          setGraphs(cached.graphs);
          if (cached.history) setHistory(cached.history);
          setOfflineEntry(cached);
          setOtherViews(null);
          setError(null);
        } else {
          setOfflineEntry(null);
          setError(errorMessage(failure.reason));
          setStatistics(null);
          setGraphs(null);
          const views = isNetworkError(failure.reason)
            ? await cachedViews(indexedDbStatisticsCache, credentials.serverUrl).catch(() => [])
            : [];
          if (!cancelled) setOtherViews(views);
        }
      }
      setLoadedKey(key);
    });
    return () => { cancelled = true; };
  }, [credentials, navigate, date, view, isToday, reloadCount]);

  function applyQuery(query = queryInput) {
    const q = query.trim();
    setQueryInput(q);
    if (q) saveRecent(q);
    setView(v => ({ ...v, query: q }));
  }

  // While offline, a view kept from before today shows its own date rather than "Today"
  const todayTitle = offlineEntry && new Date(offlineEntry.fetchedAt).toDateString() !== new Date().toDateString()
    ? `Studied on ${new Date(offlineEntry.fetchedAt).toLocaleDateString(undefined, { dateStyle: 'medium' })}`
    : isToday ? 'Today' : `Studied on ${date}`;
  const serverReachable = online && !offlineEntry;

  return (
    <div className="page">
      <Navbar onLogout={logout} />

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
        <input
          type="search"
          value={queryInput}
          onChange={e => setQueryInput(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') applyQuery(); }}
          placeholder="Search query, e.g. tag=languages"
          aria-label="Only the cards matching this query"
          style={{ ...input, flex: 1, minWidth: 180 }}
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

      {offlineEntry && (
        <div role="status" style={{ marginTop: 16, padding: '8px 12px', borderRadius: 6, border: '1px solid var(--tone-orange-border)', background: 'var(--tone-orange-bg)', color: 'var(--tone-orange-fg)', fontSize: 14 }}>
          {offlineBanner(offlineEntry.fetchedAt, pending)}
        </div>
      )}

      {error && !loading && <div style={{ color: 'var(--error)', marginTop: 16, whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>{error}</div>}
      {otherViews && !loading && (
        otherViews.length > 0 ? (
          <div style={{ marginTop: 12, fontSize: 14 }}>
            <p style={{ margin: '0 0 8px' }}>These statistics were not kept for offline use. Kept views:</p>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6, alignItems: 'flex-start' }}>
              {otherViews.map(entry => (
                <button key={entry.key} onClick={() => { setQueryInput(entry.view.query); setView(entry.view); setDate(toDateInputValue(new Date())); }}>
                  {describeView(entry.view)} <span style={{ color: 'var(--text-muted)' }}>({new Date(entry.fetchedAt).toLocaleString()})</span>
                </button>
              ))}
            </div>
          </div>
        ) : <p style={{ ...note, marginTop: 12 }}>No statistics were kept for offline use yet. They are kept each time this page loads them.</p>
      )}

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
                  <ScheduleForm key={`a${statistics.advance_safe_count}`} kind="Advance" safeCount={statistics.advance_safe_count} disabled={!serverReachable} onDone={o => { showOutcome(o); reload(); }} />
                  <ScheduleForm key={`p${statistics.postpone_safe_count}`} kind="Postpone" safeCount={statistics.postpone_safe_count} disabled={!serverReachable} onDone={o => { showOutcome(o); reload(); }} />
                  {!serverReachable && <p style={note}>{OFFLINE_HINT}</p>}
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
