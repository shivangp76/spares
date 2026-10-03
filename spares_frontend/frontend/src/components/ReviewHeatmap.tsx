import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { sectionLabel } from '../utils';

// The grid always reaches at least this far back, and this far ahead so today can be centered
const PAST_WEEKS = 52;
const FUTURE_WEEKS = 35;
const CELL = 11;
const GAP = 3;
const STEP = CELL + GAP;
const LABEL_WIDTH = 22;
const TOP = 16;
const DAY_MS = 24 * 60 * 60 * 1000;
const WEEKDAY_LABELS = ['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'];
const navButton: React.CSSProperties = { padding: '2px 10px', fontSize: 12 };
// The share of the review color mixed into the empty cell color for each level
const LEVEL_MIX = [0, 30, 55, 80, 100];
// Formatting is costly with `toLocaleDateString`, which builds a new formatter on every call
const TITLE_DATE = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });
const SELECTED_DATE = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });

interface Cell {
  key: string;
  date: Date;
  week: number;
  weekday: number;
  count: number;
  future: boolean;
}

/** `YYYY-MM-DD` for `date` in local time, the format the server keys dates by. */
function dateKey(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Local midnight of `date` moved by `days`, safe across DST changes. */
function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

function parseKey(key: string): Date {
  const [year, month, day] = key.split('-').map(Number);
  return new Date(year, month - 1, day);
}

function daysBetween(from: Date, to: Date): number {
  return Math.round((to.getTime() - from.getTime()) / DAY_MS);
}

function levelColor(level: number): string {
  return level === 0
    ? 'var(--border-subtle)'
    : `color-mix(in srgb, var(--success) ${LEVEL_MIX[level]}%, var(--border-subtle))`;
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString()} ${word}${n === 1 ? '' : 's'}`;
}

interface Summary {
  dailyAverage: number;
  daysLearnedPercent: number;
  longestStreak: number;
  currentStreak: number;
  total: number;
}

/** Statistics over the whole history, like Anki's Review Heatmap add-on shows. */
function summarize(counts: Record<string, number>, today: Date): Summary | null {
  const keys = Object.keys(counts).filter(k => counts[k] > 0).sort();
  if (keys.length === 0) return null;
  const total = keys.reduce((sum, k) => sum + counts[k], 0);
  const span = daysBetween(parseKey(keys[0]), today) + 1;

  let longestStreak = 0;
  let streak = 0;
  let previous: Date | null = null;
  for (const key of keys) {
    const date = parseKey(key);
    streak = previous && daysBetween(previous, date) === 1 ? streak + 1 : 1;
    longestStreak = Math.max(longestStreak, streak);
    previous = date;
  }

  // Not having reviewed yet today doesn't break the streak
  let day = counts[dateKey(today)] ? today : addDays(today, -1);
  let currentStreak = 0;
  while (counts[dateKey(day)]) {
    currentStreak++;
    day = addDays(day, -1);
  }

  return {
    dailyAverage: Math.round(total / span),
    daysLearnedPercent: Math.round((keys.length / span) * 100),
    longestStreak,
    currentStreak,
    total,
  };
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <div>
      <div style={{ fontSize: 18, fontWeight: 600 }}>{value}</div>
      <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>{label}</div>
    </div>
  );
}

/** Daily review counts as a scrollable calendar grid centered on today, with streak statistics. */
export default function ReviewHeatmap({ counts }: { counts: Record<string, number> }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState<Cell | null>(null);
  const [atStart, setAtStart] = useState(false);
  const [atEnd, setAtEnd] = useState(false);
  const todayKey = dateKey(new Date());

  const { cells, weeks, todayWeek, months, summary, thresholds } = useMemo(() => {
    const today = parseKey(todayKey);
    const weekStart = (date: Date) => addDays(date, -date.getDay());
    const firstKey = Object.keys(counts).sort()[0];
    const yearAgo = addDays(today, -PAST_WEEKS * 7);
    // Columns are weeks starting on Sunday, from the first review (or a year ago) into the future
    const start = weekStart(firstKey && firstKey < dateKey(yearAgo) ? parseKey(firstKey) : yearAgo);
    const todayWeek = daysBetween(start, weekStart(today)) / 7;
    const weeks = todayWeek + 1 + FUTURE_WEEKS;
    const cells: Cell[] = [];
    const months: { week: number; label: string }[] = [];
    for (let i = 0; i < weeks * 7; i++) {
      const date = addDays(start, i);
      const key = dateKey(date);
      const week = Math.floor(i / 7);
      cells.push({ key, date, week, weekday: date.getDay(), count: counts[key] ?? 0, future: date > today });
      if (date.getDate() === 1 && week < weeks - 2) {
        // Months of other years are marked with theirs, e.g. Sep '25, as the grid can span several
        const month = date.toLocaleDateString(undefined, { month: 'short' });
        const year = date.getFullYear() === today.getFullYear() ? '' : ` '${String(date.getFullYear() % 100).padStart(2, '0')}`;
        months.push({ week, label: month + year });
      }
    }
    // Levels are relative to the average of the past year's study days, so they adapt to the workload
    const active = cells.filter(c => c.count > 0 && c.date > yearAgo);
    const average = active.reduce((sum, c) => sum + c.count, 0) / Math.max(1, active.length);
    const thresholds = [0.5, 1, 1.5].map(f => f * average);
    return { cells, weeks, todayWeek, months, summary: summarize(counts, today), thresholds };
  }, [counts, todayKey]);

  const updateEnds = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setAtStart(el.scrollLeft <= 0);
    setAtEnd(el.scrollLeft + el.clientWidth >= el.scrollWidth - 1);
  }, []);

  const center = useCallback((behavior: ScrollBehavior) => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTo({ left: todayWeek * STEP + CELL / 2 - el.clientWidth / 2, behavior });
    updateEnds();
  }, [todayWeek, updateEnds]);

  /** Scrolls by most of the visible width, keeping a few weeks in view for context. */
  function page(direction: -1 | 1) {
    const el = scrollRef.current;
    if (el) el.scrollBy({ left: direction * Math.max(STEP, el.clientWidth - 4 * STEP), behavior: 'smooth' });
  }

  useLayoutEffect(() => center('instant'), [center]);

  // Built once per data change rather than on every hover, as there can be thousands of cells
  const grid = useMemo(() => {
    const level = (count: number) => {
      if (count === 0) return 0;
      const index = thresholds.findIndex(t => count <= t);
      return index === -1 ? 4 : index + 1;
    };
    return cells.map((c, i) => (
      <rect
        key={c.key}
        data-index={c.future ? undefined : i}
        x={c.week * STEP}
        y={TOP + c.weekday * STEP}
        width={CELL}
        height={CELL}
        rx={2}
        fill={levelColor(level(c.count))}
        opacity={c.future ? 0.4 : 1}
        stroke={c.key === todayKey ? 'var(--text-muted)' : 'none'}
        strokeWidth={1}
      >
        {!c.future && <title>{`${plural(c.count, 'review')} on ${TITLE_DATE.format(c.date)}`}</title>}
      </rect>
    ));
  }, [cells, thresholds, todayKey]);

  /** Selects the cell under the pointer, delegated from the grid so cells need no handlers of their own. */
  function selectTarget(event: React.MouseEvent<SVGSVGElement>) {
    const index = (event.target as Element).getAttribute('data-index');
    if (index !== null) setSelected(cells[Number(index)]);
  }

  const width = weeks * STEP - GAP;
  const height = TOP + 7 * STEP - GAP;

  return (
    <div style={{ marginBottom: 32 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8, marginBottom: 6 }}>
        <div style={{ ...sectionLabel, marginBottom: 0 }}>Reviews</div>
        <div style={{ display: 'flex', gap: 4 }}>
          <button onClick={() => page(-1)} disabled={atStart} title="Show earlier weeks" style={navButton}>Back</button>
          <button onClick={() => center('smooth')} title="Center on today" style={navButton}>Center</button>
          <button onClick={() => page(1)} disabled={atEnd} title="Show later weeks" style={navButton}>Forward</button>
        </div>
      </div>
      <div style={{ display: 'flex' }}>
        {/* Outside the scrolled area so the weekdays stay in view */}
        <svg width={LABEL_WIDTH} height={height} aria-hidden style={{ display: 'block', flexShrink: 0 }}>
          {WEEKDAY_LABELS.map((label, weekday) => (
            <text key={label} x={0} y={TOP + weekday * STEP + CELL - 2} fontSize={10} fill="var(--text-muted)">{label}</text>
          ))}
        </svg>
        <div ref={scrollRef} onScroll={updateEnds} className="heatmap-scroll" style={{ minWidth: 0, flex: 1 }}>
          <svg
            width={width}
            height={height}
            role="img"
            aria-label="Reviews per day"
            style={{ display: 'block' }}
            onMouseOver={selectTarget}
            onClick={selectTarget}
            onMouseLeave={() => setSelected(null)}
          >
            {months.map(({ week, label }) => (
              <text key={`${week}-${label}`} x={week * STEP} y={10} fontSize={10} fill="var(--text-muted)">{label}</text>
            ))}
            {grid}
            {selected && (
              <rect
                x={selected.week * STEP}
                y={TOP + selected.weekday * STEP}
                width={CELL}
                height={CELL}
                rx={2}
                fill="none"
                stroke="var(--text)"
                strokeWidth={1}
                pointerEvents="none"
              />
            )}
          </svg>
        </div>
      </div>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 12, flexWrap: 'wrap', fontSize: 12, color: 'var(--text-muted)', marginTop: 6, minHeight: 16 }}>
        <span>
          {selected
            ? `${plural(selected.count, 'review')} on ${SELECTED_DATE.format(selected.date)}`
            : summary ? `${plural(summary.total, 'review')} in total` : 'No reviews yet'}
        </span>
        <span style={{ display: 'flex', alignItems: 'center', gap: 3 }} aria-hidden>
          Less
          {LEVEL_MIX.map((_, l) => (
            <span key={l} style={{ width: CELL, height: CELL, borderRadius: 2, background: levelColor(l), display: 'inline-block' }} />
          ))}
          More
        </span>
      </div>
      {summary && (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(120px, 1fr))', gap: 12, maxWidth: 560, marginTop: 12 }}>
          <Stat value={summary.dailyAverage.toLocaleString()} label="Daily average" />
          <Stat value={`${summary.daysLearnedPercent}%`} label="Days learned" />
          <Stat value={plural(summary.longestStreak, 'day')} label="Longest streak" />
          <Stat value={plural(summary.currentStreak, 'day')} label="Current streak" />
        </div>
      )}
    </div>
  );
}
