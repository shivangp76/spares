import { useCallback, useEffect, useMemo, useState } from "react";
import { useDataVersion } from "../dataVersion";
import { useNavigate } from "react-router-dom";
import { formatMinutes, reviewQueryPath } from "../goals";
import { loginPath } from "../login";
import {
  advanceCards,
  ApiError,
  getReviewHistory,
  getStatistics,
  getStatisticsGraphs,
  getGoalStatus,
  getStepsStats,
  optimizeScheduler,
  postponeCards,
  rescheduleCards,
} from "../api/client";
import { useServerReachable } from "../api/reachability";
import ActionResult, { type ActionOutcome } from "../components/ActionResult";
import BarChart from "../components/charts/BarChart";
import {
  cumulative,
  daysBetween,
  fillDays,
  HOUR_LABELS,
  percent,
  toSeries,
  type Series,
} from "../components/charts/chartData";
import Histogram from "../components/charts/Histogram";
import RecentSearches from "../components/RecentSearches";
import SearchInput from "../components/SearchInput";
import ReviewHeatmap from "../components/ReviewHeatmap";
import { useAuth } from "../hooks/useAuth";
import { useCardStates } from "../hooks/useCardStates";
import { useRecentSearches } from "../hooks/useRecentSearches";
import { runOnServer, useLocalStatus } from "../local/copy";
import {
  type AnswerGroup,
  type GoalMilestoneStatus,
  type GoalPartStatus,
  type GoalStatus,
  type OptimizeResponse,
  type PassFail,
  type StatisticsGraphsResponse,
  type StatisticsResponse,
  type StepsGroupKind,
  type StepsStatsResponse,
} from "../types/spares";
import { errorMessage, formatDuration, td, th } from "../utils";
import PageTitle from "../components/PageTitle";
import { askConfirm } from "../ask";

/** The id of the section titled `title`, which the section bar links to. */
function sectionId(title: string): string {
  return `stat-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
}

function SectionTitle({ children }: { children: string }) {
  return (
    <h2 id={sectionId(children)} className="section-label stat-section-title">
      {children}
    </h2>
  );
}

/** Links to the page's sections, kept at the top while it scrolls, so a chart far down is near. */
function SectionBar({ titles }: { titles: (string | false | null)[] }) {
  const shown = titles.filter((t): t is string => !!t);
  if (shown.length < 2) return null;
  return (
    <nav className="stat-nav" aria-label="Sections">
      {shown.map((title) => (
        <a
          key={title}
          href={`#${sectionId(title)}`}
          onClick={(e) => {
            e.preventDefault();
            document
              .getElementById(sectionId(title))
              ?.scrollIntoView({ behavior: "smooth" });
          }}
        >
          {title}
        </a>
      ))}
    </nav>
  );
}

const tile: React.CSSProperties = {
  background: "var(--surface)",
  border: "1px solid var(--border)",
  borderRadius: 6,
  padding: "12px 20px",
  textAlign: "center",
  minWidth: 96,
};
const tiles: React.CSSProperties = {
  display: "flex",
  gap: 16,
  flexWrap: "wrap",
};
const note: React.CSSProperties = {
  fontSize: 13,
  color: "var(--text-muted)",
  margin: "8px 0 0",
};
const input: React.CSSProperties = {
  padding: "6px 10px",
  fontSize: 14,
  border: "1px solid var(--border-strong)",
  borderRadius: 4,
};
const chip: React.CSSProperties = { padding: "4px 10px", fontSize: 13 };
const numeric: React.CSSProperties = {
  ...td,
  textAlign: "right",
  fontVariantNumeric: "tabular-nums",
};
const OFFLINE_HINT =
  "The server cannot be reached, and advancing and postponing need it.";

/** What the page was asked to show, apart from the date. */
interface StatisticsView {
  query: string;
  /** `null` for all time. */
  periodDays: number | null;
  forecastDays: number;
}

const PERIODS: { label: string; days: number | null }[] = [
  { label: "1 month", days: 30 },
  { label: "3 months", days: 90 },
  { label: "1 year", days: 365 },
  { label: "All", days: null },
];
const FORECAST_DAYS = [7, 30, 90, 365];

/** `YYYY-MM-DD` for `date` in local time, the format `<input type="date">` uses. */
function toDateInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** The instant to request statistics for: now if `value` is today, otherwise local noon of that day. */
function requestDate(value: string): Date {
  if (value === toDateInputValue(new Date())) return new Date();
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, month - 1, day, 12);
}

function Tile({ value, label }: { value: React.ReactNode; label: string }) {
  return (
    <div className="tile" style={tile}>
      <div style={{ fontSize: 24, fontWeight: 600 }}>{value}</div>
      <div className="secondary-text">{label}</div>
    </div>
  );
}

function StateCounts({
  counts,
  empty,
}: {
  counts: Record<string, number>;
  empty: string;
}) {
  const { stateName } = useCardStates();
  const entries = Object.entries(counts)
    .filter(([, count]) => count > 0)
    .sort(([a], [b]) => Number(a) - Number(b));
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  if (entries.length === 0)
    return <p style={{ color: "var(--text-secondary)" }}>{empty}</p>;
  return (
    <div style={tiles}>
      {entries.map(([stateId, count]) => (
        <Tile key={stateId} value={count} label={stateName(Number(stateId))} />
      ))}
      <Tile value={total} label="Total" />
    </div>
  );
}

/** `spares card advance` / `spares card postpone`, prefilled with the number of cards that are safe to move. */
function ScheduleForm({
  kind,
  safeCount,
  disabled,
  onDone,
}: {
  kind: "Advance" | "Postpone";
  safeCount: number;
  disabled: boolean;
  onDone: (outcome: ActionOutcome) => void;
}) {
  const { credentials } = useAuth();
  const [count, setCount] = useState(String(safeCount));
  const [query, setQuery] = useState("");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    if (!credentials || disabled) return;
    const n = Number(count);
    if (!Number.isInteger(n) || n <= 0) {
      setError(`Invalid count: ${count}`);
      return;
    }
    setRunning(true);
    setError(null);
    try {
      const run = kind === "Advance" ? advanceCards : postponeCards;
      const eventId = await run(
        credentials.schedulerName,
        n,
        query.trim() || null,
      );
      onDone({
        message: `${kind === "Advance" ? "Advanced" : "Postponed"} ${n} cards.`,
        eventIds: eventId === null ? [] : [eventId],
      });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div
      style={{
        display: "flex",
        gap: 8,
        alignItems: "center",
        flexWrap: "wrap",
        fontSize: 14,
        marginTop: 8,
      }}
    >
      <input
        type="number"
        min={1}
        value={count}
        onChange={(e) => setCount(e.target.value)}
        aria-label={`Number of cards to ${kind.toLowerCase()}`}
        disabled={disabled}
        style={{ ...input, width: 80 }}
      />
      <input
        type="text"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") run();
        }}
        placeholder="Optional query, e.g. tag=a"
        disabled={disabled}
        style={{ ...input, flex: 1, minWidth: 160 }}
      />
      <button
        onClick={run}
        disabled={running || disabled}
        title={disabled ? OFFLINE_HINT : undefined}
        className="touch-target-small"
        style={{ minWidth: 90 }}
      >
        {running ? "…" : kind}
      </button>
      {error && (
        <span
          style={{
            color: "var(--error)",
            fontSize: 13,
            whiteSpace: "pre-wrap",
            fontFamily: "monospace",
          }}
        >
          {error}
        </span>
      )}
    </div>
  );
}

function TodaySection({
  statistics,
  graphs,
  title,
}: {
  statistics: StatisticsResponse | null;
  graphs: StatisticsGraphsResponse | null;
  title: string;
}) {
  const today = graphs?.today;
  return (
    <>
      <SectionTitle>{title}</SectionTitle>
      <div style={tiles}>
        {statistics && (
          <>
            <Tile
              value={statistics.cards_studied_count}
              label="Cards studied"
            />
            <Tile
              value={formatDuration(statistics.recall_duration)}
              label="Recall time"
            />
            <Tile
              value={formatDuration(statistics.rate_duration)}
              label="Rate time"
            />
            <Tile
              value={formatDuration(
                statistics.recall_duration + statistics.rate_duration,
              )}
              label="Total time"
            />
          </>
        )}
        {today && today.reviews > 0 && (
          <>
            <Tile
              value={percent(today.passed, today.reviews)}
              label="Correct"
            />
            <Tile
              value={formatDuration(Math.round(today.seconds / today.reviews))}
              label="Per card"
            />
            <Tile value={today.learning + today.relearning} label="Learning" />
            <Tile value={today.young + today.mature} label="Review" />
          </>
        )}
        {/* Ratings in filtered tags that do not reschedule, apart from the reviews; only on days with some */}
        {today && today.crams > 0 && (
          <>
            <Tile value={today.crams} label="Crams" />
            <Tile
              value={formatDuration(today.cram_seconds)}
              label="Cram time"
            />
            <Tile
              value={percent(today.crams_passed, today.crams)}
              label="Cram correct"
            />
          </>
        )}
      </div>
      {today && today.reviews > 0 && (
        <p style={note}>
          {today.reviews} reviews: {today.learning} learning, {today.relearning}{" "}
          relearning, {today.young} young, {today.mature} mature. Crams count
          apart, with their own % correct.
        </p>
      )}
    </>
  );
}

const FORECAST_SPECS = [
  { key: "learning", label: "Learning", color: "var(--chart-learning)" },
  { key: "review", label: "Review", color: "var(--chart-mature)" },
];

/** `YYYY-MM-DD` as a short date, like "May 1, 2027". */
function formatDay(day: string): string {
  return new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    timeZone: "UTC",
  });
}

/** The goal's next milestone, the first whose day is still to come, as `part` has it. */
function nextMilestone(part: GoalPartStatus): GoalMilestoneStatus | undefined {
  return part.milestones.find((milestone) => milestone.new_on_day === null);
}

/** The goal table's cells, a little narrower than other tables' so its many columns fit. */
const goalTh: React.CSSProperties = { ...th, padding: "8px 8px" };
const goalTd: React.CSSProperties = { ...td, padding: "8px 8px" };
const goalNumeric: React.CSSProperties = { ...numeric, padding: "8px 8px" };

/** `YYYY-MM-DD` as a date without its year, like "May 1", for table cells. */
function formatShortDay(day: string): string {
  return new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  });
}

/**
 * How much of a part is learned, as a thin bar under its count, with a tick where the next
 * milestone wants it to be.
 */
function LearnedBar({ part }: { part: GoalPartStatus }) {
  const studied = part.cards - part.suspended;
  const learned = studied - part.new;
  const fraction = studied === 0 ? 0 : learned / studied;
  const milestone = nextMilestone(part);
  const target =
    milestone && studied > 0
      ? Math.max(0, Math.min(1, (studied - milestone.new_left) / studied))
      : null;
  return (
    <div>
      <span style={{ fontVariantNumeric: "tabular-nums" }}>
        {learned} / {studied}
      </span>
      <div
        role="presentation"
        style={{
          height: 4,
          marginTop: 4,
          borderRadius: 2,
          background: "var(--border-subtle)",
          minWidth: 80,
          position: "relative",
        }}
      >
        <div
          style={{
            width: `${Math.round(fraction * 100)}%`,
            height: "100%",
            borderRadius: 2,
            background: "var(--chart-mature)",
          }}
        />
        {milestone && target !== null && (
          <div
            title={`${formatDay(milestone.date)}: ${studied - milestone.new_left} learned`}
            style={{
              position: "absolute",
              left: `calc(${(target * 100).toFixed(1)}% - 1px)`,
              top: -3,
              width: 2,
              height: 10,
              borderRadius: 1,
              background: "var(--text)",
            }}
          />
        )}
      </div>
    </div>
  );
}

/**
 * The goals of config.toml (`[goals.<name>]`): for each, a row per part with how much is learned,
 * the pace it needs and has, and the recall predicted on the day, then the weeks of the whole goal.
 * A part's name starts a review of its cards.
 */
function GoalsSection({ goals }: { goals: GoalStatus[] }) {
  return (
    <>
      {goals.map((goal) => (
        <GoalTable key={goal.name} goal={goal} />
      ))}
    </>
  );
}

function GoalTable({ goal }: { goal: GoalStatus }) {
  const navigate = useNavigate();
  const target = goal.retention ?? 0.9;
  const whole = goal.parts[0];
  const labels = whole?.weeks.map((week) => formatDay(week.start)) ?? [];
  const reviews: Series[] = whole
    ? [
        {
          key: "reviews",
          label: "Reviews",
          color: "var(--chart-bar)",
          values: whole.weeks.map((week) => week.reviews),
        },
      ]
    : [];
  const next = whole ? nextMilestone(whole) : undefined;
  const passed = whole?.milestones.filter((m) => m.new_on_day !== null) ?? [];
  const pace = (part: GoalPartStatus) => {
    if (part.new === 0) return "Done";
    if (part.projected_learned_by === null) return "No pace";
    const late = part.projected_learned_by > goal.learned_by;
    return (
      <span
        style={late ? { color: "var(--error)" } : undefined}
        title={formatDay(part.projected_learned_by)}
      >
        {formatShortDay(part.projected_learned_by)}
      </span>
    );
  };
  const projected = (part: GoalPartStatus) => {
    const projection = part.projection;
    if (!projection) return "–";
    if (part.new === 0) return "Done";
    const { learned_earliest: earliest, learned_latest: latest } = projection;
    if (earliest === null) return "Never";
    const late = latest === null || latest > goal.learned_by;
    const text =
      earliest === latest
        ? formatShortDay(earliest)
        : `${formatShortDay(earliest)} – ${latest === null ? "never" : formatShortDay(latest)}`;
    const full =
      earliest === latest
        ? formatDay(earliest)
        : `${formatDay(earliest)} to ${latest === null ? "never" : formatDay(latest)}`;
    return (
      <span style={late ? { color: "var(--error)" } : undefined} title={full}>
        {text}
      </span>
    );
  };
  const heldBack = goal.presets.filter(
    (preset) => preset.lost_to_links >= 0.005,
  );
  const projectedAtNext = next?.projected_new_left;
  return (
    <div style={{ marginBottom: 24 }}>
      <h3 className="stat-subheading">{goal.name}</h3>
      <p style={{ ...note, marginTop: 0 }}>
        {formatDay(goal.date)} · {goal.days_to_date} days left · new cards
        learned by {formatDay(goal.learned_by)} ({goal.days_to_learned_by} days)
        {goal.retention !== null &&
          ` · recall target ${Math.round(goal.retention * 100)}%`}
      </p>
      {whole && (
        <p
          style={{ ...note, marginTop: 0 }}
          title="The next week's new cards, with their learning steps, and the cards due, at the pace of reviews so far. The new cards' later reviews come on top."
        >
          {goal.minutes_per_day === null
            ? "Time a day at these limits unknown"
            : `~${formatMinutes(goal.minutes_per_day)}/day at these limits`}
          {" · "}
          {formatMinutes(
            (whole.weeks[whole.weeks.length - 1]?.minutes ?? 0) / 7,
          )}
          /day the last 7 days
        </p>
      )}
      {whole && (next || passed.length > 0) && (
        <p style={{ ...note, marginTop: 0 }}>
          {passed.map((milestone) => (
            <span key={milestone.date}>
              {formatDay(milestone.date)}:{" "}
              <span
                style={milestone.met ? undefined : { color: "var(--error)" }}
              >
                {milestone.met ? "met" : "missed"}
              </span>{" "}
              ({milestone.new_on_day} new, at most {milestone.new_left}) ·{" "}
            </span>
          ))}
          {next && (
            <>
              Next milestone {formatDay(next.date)} ({next.days_left} days): at
              most {next.new_left} new, now {whole.new} ·{" "}
              {(next.new_per_day_needed ?? 0).toFixed(1)}/day needed, last 14
              days {(whole.learned_recently / 14).toFixed(1)}/day
              {next.on_track === false && (
                <span style={{ color: "var(--error)" }}> · behind</span>
              )}
              {projectedAtNext && (
                <>
                  {" "}
                  · projected{" "}
                  <span
                    style={
                      projectedAtNext[1] > next.new_left
                        ? { color: "var(--error)" }
                        : undefined
                    }
                  >
                    {projectedAtNext[0] === projectedAtNext[1]
                      ? projectedAtNext[0]
                      : `${projectedAtNext[0]}–${projectedAtNext[1]}`}{" "}
                    new left then
                  </span>
                </>
              )}
            </>
          )}
        </p>
      )}
      <div style={{ overflowX: "auto" }}>
        <table style={{ borderCollapse: "collapse", fontSize: 14 }}>
          <thead>
            <tr>
              <th style={goalTh}>Part</th>
              <th style={goalTh}>Learned</th>
              <th style={{ ...goalTh, textAlign: "right" }}>New</th>
              <th style={{ ...goalTh, textAlign: "right" }}>Learning</th>
              <th
                style={{ ...goalTh, textAlign: "right" }}
                title="New cards a day that learn the rest in time"
              >
                Needed / day
              </th>
              {next && (
                <th
                  style={{ ...goalTh, textAlign: "right" }}
                  title={`New cards a day that reach this part's share of the ${formatDay(next.date)} milestone`}
                >
                  By{" "}
                  {new Date(`${next.date}T00:00:00Z`).toLocaleDateString(
                    undefined,
                    { month: "short", day: "numeric", timeZone: "UTC" },
                  )}
                </th>
              )}
              <th
                style={{ ...goalTh, textAlign: "right" }}
                title="Cards first studied in the last 14 days"
              >
                Last 14 days
              </th>
              <th
                style={goalTh}
                title="When the new cards would all be learned at the last 14 days' pace"
              >
                At this pace
              </th>
              <th
                style={goalTh}
                title="When the new cards would all be learned with today's limits, orders, sibling burying and linked notes, studying every day"
              >
                Projected
              </th>
              <th
                style={{ ...goalTh, textAlign: "right" }}
                title="The average recall predicted on the day, if the studied cards were not reviewed again"
              >
                Recall on the day
              </th>
              <th
                style={{ ...goalTh, textAlign: "right" }}
                title="Studied cards that would be below the target on the day without another review"
              >
                Below {Math.round(target * 100)}%
              </th>
            </tr>
          </thead>
          <tbody>
            {goal.parts.map((part, i) => (
              <tr
                key={part.name}
                style={i === 0 ? { fontWeight: 600 } : undefined}
              >
                <td style={{ ...goalTd, whiteSpace: "nowrap" }}>
                  <a
                    href={reviewQueryPath(part.query)}
                    onClick={(e) => {
                      e.preventDefault();
                      navigate(reviewQueryPath(part.query));
                    }}
                    title={`Review the cards of ${part.name} due today`}
                    style={{
                      color: "inherit",
                      textDecorationColor: "var(--accent)",
                      textUnderlineOffset: 3,
                    }}
                  >
                    {part.name}
                  </a>
                </td>
                <td style={goalTd}>
                  <LearnedBar part={part} />
                </td>
                <td style={goalNumeric}>{part.new}</td>
                <td style={goalNumeric}>{part.learning}</td>
                <td style={goalNumeric}>
                  {part.new_per_day_needed === null
                    ? "–"
                    : part.new_per_day_needed.toFixed(1)}
                </td>
                {next && <MilestoneNeed part={part} />}
                <td style={goalNumeric}>{part.learned_recently}</td>
                <td style={{ ...goalTd, whiteSpace: "nowrap" }}>
                  {pace(part)}
                </td>
                <td style={{ ...goalTd, whiteSpace: "nowrap" }}>
                  {projected(part)}
                </td>
                <td style={goalNumeric}>
                  {part.mean_recall_on_date === null
                    ? "–"
                    : `${Math.round(part.mean_recall_on_date * 100)}%`}
                </td>
                <td style={goalNumeric}>{part.below_retention_on_date}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {heldBack.length > 0 && (
        <p style={{ ...note, marginTop: 8 }}>
          Limits unused while new cards wait on linked notes, until{" "}
          {formatDay(goal.learned_by)}:{" "}
          {heldBack
            .map(
              (preset) =>
                `${preset.name} ${Math.round(preset.lost_to_links * 100)}% of ${preset.new_limit}/day`,
            )
            .join(", ")}
        </p>
      )}
      {whole && whole.weeks.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <BarChart
            title={`${goal.name}: reviews and cards learned by week`}
            labels={labels}
            series={reviews}
            labelHeading="Week of"
            mergeToFit={false}
            line={{
              label: "Learned",
              color: "var(--chart-line)",
              values: whole.weeks.map((week) => week.learned),
              merge: "last",
            }}
          />
        </div>
      )}
      {whole && whole.weeks.length > 0 && (
        <div style={{ marginTop: 12 }}>
          <p style={{ ...note, margin: "0 0 4px" }}>
            Minutes spent on its cards by week
          </p>
          <BarChart
            title={`${goal.name}: minutes by week`}
            labels={labels}
            series={[
              {
                key: "minutes",
                label: "Minutes",
                color: "var(--chart-bar)",
                values: whole.weeks.map((week) => Math.round(week.minutes)),
              },
            ]}
            labelHeading="Week of"
            mergeToFit={false}
            height={80}
          />
        </div>
      )}
    </div>
  );
}

/** The pace `part` needs for the next milestone, in red when the last 14 days' falls short. */
function MilestoneNeed({ part }: { part: GoalPartStatus }) {
  const milestone = nextMilestone(part);
  const need = milestone?.new_per_day_needed;
  return (
    <td
      style={{
        ...goalNumeric,
        ...(milestone?.on_track === false ? { color: "var(--error)" } : {}),
      }}
      title={
        milestone
          ? `At most ${milestone.new_left} new by ${formatDay(milestone.date)}`
          : undefined
      }
    >
      {need === undefined || need === null ? "–" : need.toFixed(1)}
    </td>
  );
}

function ForecastSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const { labels, series, total } = useMemo(() => {
    const series = toSeries(
      graphs.forecast.map((day) => [day.learning, day.review]),
      FORECAST_SPECS,
    );
    return {
      labels: graphs.forecast.map((day) => day.date),
      series,
      total: graphs.forecast.reduce(
        (sum, day) => sum + day.learning + day.review,
        0,
      ),
    };
  }, [graphs]);
  const line = useMemo(
    () => ({
      label: "Cumulative",
      color: "var(--chart-line)",
      values: cumulative(
        graphs.forecast.map((day) => day.learning + day.review),
      ),
      merge: "last" as const,
    }),
    [graphs],
  );
  if (total === 0)
    return <p style={{ color: "var(--text-secondary)" }}>Nothing due.</p>;
  const today = graphs.forecast[0];
  return (
    <>
      <BarChart
        title="Cards due per day"
        labels={labels}
        series={series}
        line={line}
      />
      <p style={note}>
        {total.toLocaleString()} due in the next {graphs.forecast.length} days,{" "}
        {((today?.learning ?? 0) + (today?.review ?? 0)).toLocaleString()} of
        them today (overdue included). New and suspended cards are left out.
      </p>
    </>
  );
}

const REVIEW_SPECS = [
  { key: "learning", label: "Learning", color: "var(--chart-learning)" },
  { key: "relearning", label: "Relearning", color: "var(--chart-relearning)" },
  { key: "young", label: "Young", color: "var(--chart-young)" },
  { key: "mature", label: "Mature", color: "var(--chart-mature)" },
  { key: "cram", label: "Cram", color: "var(--chart-cram)" },
];

function ReviewsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const [showTime, setShowTime] = useState(false);
  const days = useMemo(
    () => daysBetween(graphs.period_start, graphs.date),
    [graphs],
  );
  const counts = useMemo(
    () =>
      toSeries(
        fillDays(
          days,
          graphs.reviews,
          (d) => [d.learning, d.relearning, d.young, d.mature, d.cram],
          5,
        ),
        REVIEW_SPECS,
      ),
    [days, graphs],
  );
  const minutes: Series[] = useMemo(
    () => [
      {
        key: "minutes",
        label: "Minutes",
        color: "var(--chart-bar)",
        values: fillDays(days, graphs.reviews, (d) => [d.seconds / 60], 1).map(
          ([m]) => m,
        ),
      },
    ],
    [days, graphs],
  );
  const summary = graphs.reviews_summary;
  if (summary.reviews + summary.crams === 0)
    return (
      <p style={{ color: "var(--text-secondary)" }}>
        No reviews in this period.
      </p>
    );
  return (
    <>
      <div style={{ display: "flex", gap: 4, marginBottom: 8 }}>
        <button
          style={chip}
          aria-pressed={!showTime}
          onClick={() => setShowTime(false)}
        >
          Count
        </button>
        <button
          style={chip}
          aria-pressed={showTime}
          onClick={() => setShowTime(true)}
        >
          Time
        </button>
      </div>
      {showTime ? (
        <BarChart
          title="Minutes studied per day"
          labels={days}
          series={minutes}
        />
      ) : (
        <BarChart title="Reviews per day" labels={days} series={counts} />
      )}
      <div style={{ ...tiles, marginTop: 12 }}>
        <Tile value={summary.reviews.toLocaleString()} label="Reviews" />
        {summary.crams > 0 && (
          <Tile value={summary.crams.toLocaleString()} label="Crams" />
        )}
        <Tile value={summary.daily_average.toFixed(1)} label="Daily average" />
        <Tile
          value={`${summary.days_studied}/${summary.days}`}
          label="Days studied"
        />
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
    {
      passed: row.young.passed + row.mature.passed,
      failed: row.young.failed + row.mature.failed,
    },
  ];
  return (
    <>
      <div className="table-scroll">
        <table className="data-table">
          <thead>
            <tr>
              <th style={th} rowSpan={2} />
              <th style={{ ...th, textAlign: "center" }} colSpan={3}>
                Young
              </th>
              <th style={{ ...th, textAlign: "center" }} colSpan={3}>
                Mature (≥ {graphs.mature_threshold_days} days)
              </th>
              <th style={{ ...th, textAlign: "center" }} colSpan={3}>
                Total
              </th>
            </tr>
            <tr>
              {[0, 1, 2].flatMap((g) =>
                ["Pass", "Fail", "Retention"].map((h) => (
                  <th
                    key={`${g}${h}`}
                    style={{ ...th, textAlign: "right", fontWeight: 500 }}
                  >
                    {h}
                  </th>
                )),
              )}
            </tr>
          </thead>
          <tbody>
            {graphs.true_retention.map((row) => (
              <tr key={row.label}>
                <td style={{ ...td, whiteSpace: "nowrap" }}>{row.label}</td>
                {groups(row).flatMap((g, i) => [
                  <td key={`${i}p`} style={numeric}>
                    {g.passed.toLocaleString()}
                  </td>,
                  <td key={`${i}f`} style={numeric}>
                    {g.failed.toLocaleString()}
                  </td>,
                  <td key={`${i}r`} style={{ ...numeric, fontWeight: 600 }}>
                    {rate(g)}
                  </td>,
                ])}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p style={note}>
        Reviews of cards in the review state that were passed. Learning steps
        and crams are left out.
      </p>
    </>
  );
}

function CardCountsSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const { stateName, stateRole } = useCardStates();
  const counts = graphs.card_counts;
  if (counts.total === 0)
    return <p style={{ color: "var(--text-secondary)" }}>No cards.</p>;
  const states = Object.entries(counts.by_state)
    .filter(([, n]) => n > 0)
    .sort(([a], [b]) => Number(a) - Number(b));
  // Young and mature count the cards of every review state, in place of them
  const firstReview = states.find(
    ([state]) => stateRole(Number(state)) === "Review",
  )?.[0];
  return (
    <div style={tiles}>
      {states.flatMap(([state, n]) =>
        stateRole(Number(state)) !== "Review"
          ? [<Tile key={state} value={n} label={stateName(Number(state))} />]
          : state === firstReview
            ? [
                <Tile key="young" value={counts.young} label="Young" />,
                <Tile
                  key="mature"
                  value={counts.mature}
                  label={`Mature (≥ ${graphs.mature_threshold_days}d)`}
                />,
              ]
            : [],
      )}
      <Tile value={counts.suspended} label="Suspended" />
      <Tile value={counts.buried} label="Buried" />
      <Tile value={counts.total} label="Total" />
    </div>
  );
}

function AnswerButtonsSection({
  graphs,
}: {
  graphs: StatisticsGraphsResponse;
}) {
  const { ratings, learning, young, mature } = graphs.answer_buttons;
  const rows: [string, AnswerGroup][] = [
    ["Learning", learning],
    ["Young", young],
    ["Mature", mature],
  ];
  return (
    <div className="table-scroll">
      <table className="data-table">
        <thead>
          <tr>
            <th style={th} />
            {ratings.map((r) => (
              <th key={r.id} style={{ ...th, textAlign: "right" }}>
                {r.description}
              </th>
            ))}
            <th style={{ ...th, textAlign: "right" }}>Correct</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(([label, group]) => (
            <tr key={label}>
              <td style={td}>{label}</td>
              {group.counts.map(([id, count]) => (
                <td key={id} style={numeric}>
                  {count.toLocaleString()}{" "}
                  <span style={{ color: "var(--text-muted)" }}>
                    ({percent(count, group.total)})
                  </span>
                  <div
                    aria-hidden
                    style={{
                      height: 4,
                      borderRadius: 2,
                      background: "var(--chart-bar)",
                      width: `${group.total ? (count / group.total) * 100 : 0}%`,
                      marginLeft: "auto",
                      marginTop: 2,
                    }}
                  />
                </td>
              ))}
              <td style={{ ...numeric, fontWeight: 600 }}>
                {percent(group.passed, group.total)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function HourlySection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const [showCrams, setShowCrams] = useState(false);
  const hasCrams = graphs.hourly.some((h) => h.crams > 0);
  const series: Series[] = useMemo(
    () => [
      {
        key: "reviews",
        label: "Reviews",
        color: "var(--chart-bar)",
        values: graphs.hourly.map((h) => h.reviews),
      },
      ...(showCrams
        ? [
            {
              key: "crams",
              label: "Crams",
              color: "var(--chart-cram)",
              values: graphs.hourly.map((h) => h.crams),
            },
          ]
        : []),
    ],
    [graphs, showCrams],
  );
  const line = useMemo(
    () => ({
      label: showCrams ? "% correct (reviews and crams apart)" : "% correct",
      color: "var(--chart-line)",
      values: graphs.hourly.map((h) =>
        h.reviews ? (h.passed / h.reviews) * 100 : null,
      ),
      format: (v: number) => `${Math.round(v)}%`,
      max: 100,
    }),
    [graphs, showCrams],
  );
  if (graphs.hourly.every((h) => h.reviews + h.crams === 0))
    return (
      <p style={{ color: "var(--text-secondary)" }}>
        No reviews in this period.
      </p>
    );
  return (
    <>
      {hasCrams && (
        <label
          style={{
            fontSize: 13,
            display: "inline-flex",
            gap: 6,
            alignItems: "center",
            marginBottom: 8,
          }}
        >
          <input
            type="checkbox"
            checked={showCrams}
            onChange={(e) => setShowCrams(e.target.checked)}
          />
          Show crams
        </label>
      )}
      <BarChart
        title="Reviews by hour"
        labels={HOUR_LABELS}
        series={series}
        line={line}
        labelHeading="Hour"
        mergeToFit={false}
      />
      {showCrams && (
        <p style={note}>
          Crams correct:{" "}
          {graphs.hourly
            .map((h, i) =>
              h.crams
                ? `${HOUR_LABELS[i]}h ${percent(h.crams_passed, h.crams)}`
                : null,
            )
            .filter(Boolean)
            .join(", ")}
        </p>
      )}
    </>
  );
}

function AddedSection({ graphs }: { graphs: StatisticsGraphsResponse }) {
  const days = useMemo(
    () => daysBetween(graphs.period_start, graphs.date),
    [graphs],
  );
  const series: Series[] = useMemo(
    () => [
      {
        key: "added",
        label: "Cards",
        color: "var(--chart-bar)",
        values: fillDays(days, graphs.added, (d) => [d.cards], 1).map(
          ([n]) => n,
        ),
      },
    ],
    [days, graphs],
  );
  const total = graphs.added.reduce((sum, d) => sum + d.cards, 0);
  if (total === 0)
    return (
      <p style={{ color: "var(--text-secondary)" }}>
        No cards added in this period.
      </p>
    );
  return (
    <>
      <BarChart
        title="Cards added per day"
        labels={days}
        series={series}
        height={100}
      />
      <p style={note}>
        {total.toLocaleString()} cards,{" "}
        {(total / Math.max(1, days.length)).toFixed(1)} a day.
      </p>
    </>
  );
}

const formatPercent = (v: number) => `${Math.round(v * 100)}%`;
const formatAverage = (v: number) =>
  v >= 100 ? Math.round(v).toLocaleString() : v.toFixed(2);

function DistributionsSection({
  graphs,
}: {
  graphs: StatisticsGraphsResponse;
}) {
  const d = graphs.distributions;
  const average = (mean: number | null, unit = "") =>
    mean === null ? null : (
      <p style={note}>
        Average {formatAverage(mean)}
        {unit}
      </p>
    );
  return (
    <>
      <h3 className="stat-subheading">Interval</h3>
      <Histogram title="Cards by interval" data={d.interval} unit=" days" />
      {average(d.interval.mean, " days")}
      <h3 className="stat-subheading">Stability</h3>
      <Histogram title="Cards by stability" data={d.stability} unit=" days" />
      {average(d.stability.mean, " days")}
      <h3 className="stat-subheading">Difficulty</h3>
      <Histogram title="Cards by difficulty" data={d.difficulty} />
      {average(d.difficulty.mean)}
      {d.retrievability && (
        <>
          <h3 className="stat-subheading">Retrievability</h3>
          <Histogram
            title="Cards by retrievability"
            data={d.retrievability}
            format={formatPercent}
          />
          <div style={{ ...tiles, marginTop: 12 }}>
            {d.retrievability.mean !== null && (
              <Tile
                value={formatPercent(d.retrievability.mean)}
                label="Average retrievability"
              />
            )}
            {d.total_knowledge !== null && (
              <Tile
                value={Math.round(d.total_knowledge).toLocaleString()}
                label="Estimated total knowledge"
              />
            )}
          </div>
          <p style={note}>
            Total knowledge is the sum of every card’s retrievability: about how
            many of them you would recall now.
          </p>
        </>
      )}
      <h3 className="stat-subheading">Cards in review</h3>
      <div style={tiles}>
        <Tile
          value={formatAverage(d.daily_load)}
          label="Daily load (reviews a day)"
        />
        {d.average_retrievability !== null && (
          <Tile
            value={formatPercent(d.average_retrievability)}
            label="Average retrievability"
          />
        )}
        {d.average_stability !== null && (
          <Tile
            value={`${formatAverage(d.average_stability)} days`}
            label="Average stability"
          />
        )}
      </div>
      <p style={note}>
        Over the cards in review, leaving out learning and suspended cards. The
        daily load is about how many reviews a day they will cost once the
        collection settles: the sum of 1 / interval in days, an interval under a
        day counting as one.
      </p>
    </>
  );
}

const STEPS_GROUP_LABELS: Record<StepsGroupKind, string> = {
  again: "Again",
  hard: "Hard",
  good: "Good",
  again_then_good: "Again, then Good",
  relearning: "Lapse (Again)",
};

/** A step at a useful precision: `45s`, `12m`, `2.5h` or `1.3d`. */
function formatStep(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `${(seconds / 3600).toFixed(1)}h`;
  return `${(seconds / 86400).toFixed(1)}d`;
}

const formatSteps = (steps: (number | null)[]) =>
  steps.length === 0
    ? "none"
    : steps.map((step) => (step === null ? "?" : formatStep(step))).join(" ");

/** Steps stats for one preset's cards, fetched on their own since they need no query or date. */
function StepsSection({
  periodDays,
  enabled,
}: {
  periodDays: number | null;
  enabled: boolean;
}) {
  // '' for the cards that use no preset
  const [preset, setPreset] = useState("");
  const [stats, setStats] = useState<StepsStatsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    getStepsStats({ preset: preset || null, period_days: periodDays })
      .then((s) => {
        if (!cancelled) {
          setStats(s);
          setError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [preset, periodDays, enabled]);

  if (!enabled && !stats)
    return (
      <p style={note}>The server cannot be reached, and the steps need it.</p>
    );
  const retention = stats ? formatPercent(stats.desired_retention) : "";
  const r = stats?.recommended;
  return (
    <>
      <label style={{ fontSize: 14 }}>
        Preset{" "}
        <select
          value={preset}
          onChange={(e) => setPreset(e.target.value)}
          style={input}
          aria-label="Preset"
        >
          <option value="">No preset</option>
          {stats?.presets.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </label>
      {error && (
        <div style={{ color: "var(--error)", marginTop: 8 }}>{error}</div>
      )}
      {stats && r && (
        <>
          <div style={{ overflowX: "auto", marginTop: 12 }}>
            <table style={{ borderCollapse: "collapse", fontSize: 14 }}>
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
                {stats.groups.map((g) => (
                  <tr key={g.kind}>
                    <td style={td}>{STEPS_GROUP_LABELS[g.kind]}</td>
                    <td style={numeric}>
                      {g.reviews}
                      {g.reviews > 0 && g.reviews < stats.min_reviews
                        ? " (too few)"
                        : ""}
                    </td>
                    <td style={numeric}>
                      {g.reviews > 0
                        ? formatPercent(g.recalled / g.reviews)
                        : "–"}
                    </td>
                    <td style={numeric}>
                      {g.stability === null
                        ? "–"
                        : formatStep(g.stability * 86400)}
                    </td>
                    <td style={numeric}>
                      {g.interval === null ? "–" : formatStep(g.interval)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <table
            style={{ borderCollapse: "collapse", fontSize: 14, marginTop: 12 }}
          >
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
                <td style={td}>
                  {formatSteps([r.first_learning_step, r.second_learning_step])}
                </td>
              </tr>
              <tr>
                <td style={td}>Relearning steps</td>
                <td style={td}>{formatSteps(stats.relearning_steps)}</td>
                <td style={td}>{formatSteps([r.relearning_step])}</td>
              </tr>
            </tbody>
          </table>
          <p style={note}>
            Each review in a step is paired with the card’s next review within a
            day, and each group is fitted the stability that best predicts which
            were recalled. A step is the time recall takes to fall to{" "}
            {retention}. ? means fewer than {stats.min_reviews} reviews in its
            groups; a step of a day or more can go. Not affected by the search
            query.
          </p>
        </>
      )}
    </>
  );
}

const SCHEDULER_OFFLINE_HINT =
  "The server cannot be reached, and optimizing and rescheduling run on it.";

/** What a reschedule did, with Undo for its event (which a plain undo skips). */
function rescheduledOutcome(
  message: string,
  eventId: number | null,
  local: boolean,
  syncError: string | null,
): ActionOutcome {
  let text = eventId === null ? `${message} No card changed.` : message;
  if (syncError)
    text += ` The cards reach this device at its next sync (${syncError}).`;
  else if (local && eventId !== null)
    text +=
      " Undo it from a device without a copy, or with `spares event undo`.";
  return {
    message: text,
    eventIds: local || eventId === null ? [] : [eventId],
  };
}

const formatLogLoss = (v: number) => v.toFixed(4);
const formatRmse = (v: number) => `${(v * 100).toFixed(2)}%`;

/**
 * `spares card optimize` and `spares card reschedule`: fits the scheduler's parameters to the
 * review history, and applies them, or other changed scheduler settings, to every card.
 */
function SchedulerSection({
  enabled,
  onChanged,
}: {
  enabled: boolean;
  /** After the cards changed, or an undo of that */
  onChanged: () => void;
}) {
  const { credentials } = useAuth();
  const [optimized, setOptimized] = useState<OptimizeResponse | null>(null);
  // Numbered so each new outcome gets a fresh ActionResult (and Undo button)
  const [outcome, setOutcome] = useState<{
    seq: number;
    outcome: ActionOutcome;
  } | null>(null);
  const onDone = (o: ActionOutcome) => {
    setOutcome((prev) => ({ seq: (prev?.seq ?? 0) + 1, outcome: o }));
    onChanged();
  };
  const [running, setRunning] = useState<
    "optimize" | "apply" | "reschedule" | null
  >(null);
  const [error, setError] = useState<string | null>(null);

  async function run<T>(
    kind: "optimize" | "apply" | "reschedule",
    action: () => Promise<T>,
  ) {
    setRunning(kind);
    setError(null);
    try {
      return await action();
    } catch (e) {
      setError(errorMessage(e));
      return null;
    } finally {
      setRunning(null);
    }
  }

  async function optimize() {
    if (!credentials) return;
    const res = await run("optimize", () =>
      runOnServer(() => optimizeScheduler(credentials.schedulerName, false)),
    );
    if (res) setOptimized(res.result);
  }

  async function apply() {
    if (!credentials) return;
    if (
      !(await askConfirm({
        title: "Apply optimized parameters?",
        message:
          "Optimize again, and if the parameters still predict your reviews better, save them to config.toml and recompute the memory state and due date of every card, suspended and buried ones included.\n\nUndo restores the cards, but not the parameters.",
        confirmLabel: "Optimize and apply",
      }))
    )
      return;
    const res = await run("apply", () =>
      runOnServer(() => optimizeScheduler(credentials.schedulerName, true)),
    );
    if (!res) return;
    setOptimized(res.result);
    if (res.result.applied)
      onDone(
        rescheduledOutcome(
          "Saved the optimized parameters and rescheduled every card.",
          res.result.event_id,
          res.local,
          res.syncError,
        ),
      );
    else
      onDone({
        message:
          "Kept the current parameters: the optimized ones do not predict your reviews better.",
        eventIds: [],
      });
  }

  async function reschedule() {
    if (!credentials) return;
    if (
      !(await askConfirm({
        title: "Reschedule every card?",
        message:
          "Recompute the memory state and due date of every card from its review history, suspended and buried ones included.\n\nUndo restores the cards' previous schedule.",
        confirmLabel: "Reschedule",
      }))
    )
      return;
    const res = await run("reschedule", () =>
      runOnServer(() => rescheduleCards(credentials.schedulerName)),
    );
    if (res)
      onDone(
        rescheduledOutcome(
          "Rescheduled every card.",
          res.result.event_id,
          res.local,
          res.syncError,
        ),
      );
  }

  const better =
    optimized !== null &&
    optimized.optimized.log_loss < optimized.current.log_loss;
  const busy = running !== null || !enabled;
  return (
    <>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
        <button
          onClick={optimize}
          disabled={busy}
          title={enabled ? undefined : SCHEDULER_OFFLINE_HINT}
          className="touch-target-small"
        >
          {running === "optimize" ? "Optimizing…" : "Optimize parameters"}
        </button>
        <button
          onClick={reschedule}
          disabled={busy}
          title={enabled ? undefined : SCHEDULER_OFFLINE_HINT}
          className="touch-target-small"
        >
          {running === "reschedule" ? "Rescheduling…" : "Reschedule all cards"}
        </button>
      </div>
      {running === "optimize" || running === "apply" ? (
        <p style={note}>This can take a minute on a large collection.</p>
      ) : null}
      {error && (
        <div
          style={{
            color: "var(--error)",
            marginTop: 8,
            whiteSpace: "pre-wrap",
          }}
        >
          {error}
        </div>
      )}
      {!enabled && <p style={note}>{SCHEDULER_OFFLINE_HINT}</p>}
      {outcome && (
        <ActionResult
          key={outcome.seq}
          outcome={outcome.outcome}
          onReverted={onChanged}
        />
      )}
      {optimized && (
        <div style={{ marginTop: 12 }}>
          <table style={{ borderCollapse: "collapse", fontSize: 14 }}>
            <thead>
              <tr>
                <th style={th}>Parameters</th>
                <th style={th}>Log loss</th>
                <th style={th}>RMSE</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td style={td}>Current</td>
                <td style={numeric}>
                  {formatLogLoss(optimized.current.log_loss)}
                </td>
                <td style={numeric}>
                  {formatRmse(optimized.current.rmse_bins)}
                </td>
              </tr>
              <tr>
                <td style={td}>Optimized</td>
                <td style={numeric}>
                  {formatLogLoss(optimized.optimized.log_loss)}
                </td>
                <td style={numeric}>
                  {formatRmse(optimized.optimized.rmse_bins)}
                </td>
              </tr>
            </tbody>
          </table>
          <p style={note}>
            Fitted to {optimized.review_count.toLocaleString()} reviews of{" "}
            {optimized.card_count.toLocaleString()} cards. Lower is better for
            both.{" "}
            {optimized.applied
              ? "The optimized parameters are saved, and every card was rescheduled with them."
              : better
                ? "The optimized parameters predict your reviews better."
                : "The current parameters predict your reviews at least as well, so there is nothing to apply."}
          </p>
          <details style={{ marginTop: 8, fontSize: 13 }}>
            <summary style={{ cursor: "pointer" }}>Parameters</summary>
            <pre style={{ whiteSpace: "pre-wrap", margin: "8px 0 0" }}>
              {`current   = [${optimized.current_parameters.join(", ")}]\noptimized = [${optimized.optimized_parameters.join(", ")}]`}
            </pre>
          </details>
          {better && !optimized.applied && (
            <button
              onClick={apply}
              disabled={busy}
              className="touch-target-small"
              style={{ marginTop: 8 }}
            >
              {running === "apply" ? "Applying…" : "Apply and reschedule"}
            </button>
          )}
        </div>
      )}
      <p style={note}>
        Changing the scheduler, its parameters, learning steps or easy days in
        config.toml only affects cards as they are next rated. Reschedule
        applies the change to every card now. Once you have a few hundred
        reviews, optimizing fits the parameters to your own history, which
        usually predicts your memory better; again every month or so keeps them
        in step. Not affected by the search query.
      </p>
    </>
  );
}

/** Each part of the page settles independently. Ignore an older response after a filter change. */
function useStatisticsRequest<T>(
  fetchData: () => Promise<T>,
  enabled: boolean,
  revision: number,
) {
  const [result, setResult] = useState<{
    fetchData: typeof fetchData;
    revision: number;
    data: T | null;
    error: unknown;
  } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    fetchData().then(
      (data) => {
        if (active) setResult({ fetchData, revision, data, error: null });
      },
      (error: unknown) => {
        if (active) setResult({ fetchData, revision, data: null, error });
      },
    );
    return () => {
      active = false;
    };
  }, [fetchData, enabled, revision]);
  const loading =
    result?.fetchData !== fetchData || result.revision !== revision;
  return {
    data: result?.data ?? null,
    loading,
    error: loading ? null : result.error,
  };
}

export default function StatisticsPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const online = useServerReachable();
  const local = useLocalStatus();
  const {
    recent,
    save: saveRecent,
    remove: removeRecent,
  } = useRecentSearches("spares_statistics_recent_searches");

  const [date, setDate] = useState(() => toDateInputValue(new Date()));
  const [queryInput, setQueryInput] = useState("");
  const [view, setView] = useState<StatisticsView>({
    query: "",
    periodDays: 30,
    forecastDays: 30,
  });
  // Bumped to refetch after an action changes the schedule
  const [reloadCount, setReloadCount] = useState(0);
  // Fetch again after an undo, redo or sync elsewhere changed what is shown
  const dataVersion = useDataVersion();
  // Numbered so each new outcome gets a fresh ActionResult (and Undo button)
  const [scheduleOutcome, setScheduleOutcome] = useState<{
    seq: number;
    outcome: ActionOutcome;
  } | null>(null);
  const showOutcome = (outcome: ActionOutcome) =>
    setScheduleOutcome((prev) => ({ seq: (prev?.seq ?? 0) + 1, outcome }));
  const isToday = date === toDateInputValue(new Date());
  const reload = () => setReloadCount((c) => c + 1);

  useEffect(() => {
    if (!credentials) navigate(loginPath());
  }, [credentials, navigate]);

  const query = view.query.trim() || null;
  const scheduler = credentials?.schedulerName ?? "";
  const enabled = Boolean(credentials && date);
  const revision = reloadCount + dataVersion;
  const todayRequest = useCallback(
    () => getStatistics(scheduler, requestDate(date)),
    [scheduler, date],
  );
  const graphsRequest = useCallback(
    () =>
      getStatisticsGraphs(scheduler, {
        query,
        periodDays: view.periodDays,
        forecastDays: view.forecastDays,
        date: requestDate(date),
      }),
    [scheduler, query, view.periodDays, view.forecastDays, date],
  );
  const historyRequest = useCallback(() => getReviewHistory(query), [query]);
  const goalsRequest = useCallback(() => getGoalStatus(), []);
  const todayResult = useStatisticsRequest(todayRequest, enabled, revision);
  const graphResult = useStatisticsRequest(graphsRequest, enabled, revision);
  const historyResult = useStatisticsRequest(historyRequest, enabled, revision);
  // Goals are about today and the server's config.toml, so they are shown as they are, and left
  // out where they can't be fetched, as offline.
  const goalsResult = useStatisticsRequest(
    goalsRequest,
    enabled && isToday,
    revision,
  );
  const goals = goalsResult.data?.length ? goalsResult.data : null;
  const loading =
    todayResult.loading || graphResult.loading || historyResult.loading;
  const failure = todayResult.error ?? graphResult.error;
  const error = failure == null ? null : errorMessage(failure);
  const searchFailure = graphResult.error ?? historyResult.error;
  const queryError =
    query && searchFailure instanceof ApiError && searchFailure.span
      ? { query, span: searchFailure.span }
      : null;
  const statistics = todayResult.data;
  const graphs = graphResult.data;
  const history = historyResult.data;

  function applyQuery(query = queryInput) {
    const q = query.trim();
    setQueryInput(q);
    if (q) saveRecent(q);
    if (q === view.query) {
      reload();
      return;
    }
    setView((v) => ({ ...v, query: q }));
  }

  const todayTitle = isToday ? "Today" : `Studied on ${date}`;
  // A device's copy schedules cards itself, offline too
  const canSchedule = local.active || online;

  return (
    <div className="page">
      <PageTitle>Statistics</PageTitle>
      <div
        style={{
          display: "flex",
          gap: 8,
          alignItems: "center",
          flexWrap: "wrap",
        }}
      >
        <SearchInput
          value={queryInput}
          onChange={setQueryInput}
          onSubmit={applyQuery}
          placeholder="Search query, e.g. tag=languages"
          ariaLabel="Only the cards matching this query"
          error={queryError}
          style={{ minWidth: 180, maxWidth: "none" }}
          inputStyle={input}
        />
        <button onClick={() => applyQuery()} className="touch-target-small">
          Apply
        </button>
        {view.query && (
          <button
            onClick={() => applyQuery("")}
            className="touch-target-small"
            title="Every card"
          >
            Clear
          </button>
        )}
      </div>
      <RecentSearches
        recent={recent}
        onSelect={applyQuery}
        onRemove={removeRecent}
        style={{ marginTop: 8 }}
      />

      <div
        style={{
          display: "flex",
          gap: 16,
          alignItems: "center",
          flexWrap: "wrap",
          marginTop: 8,
          fontSize: 14,
        }}
      >
        <div
          role="group"
          aria-label="Period"
          style={{ display: "flex", gap: 4 }}
        >
          {PERIODS.map((p) => (
            <button
              key={p.label}
              style={chip}
              aria-pressed={view.periodDays === p.days}
              onClick={() => setView((v) => ({ ...v, periodDays: p.days }))}
            >
              {p.label}
            </button>
          ))}
        </div>
        <label>
          Forecast{" "}
          <select
            value={view.forecastDays}
            onChange={(e) =>
              setView((v) => ({ ...v, forecastDays: Number(e.target.value) }))
            }
            style={input}
          >
            {FORECAST_DAYS.map((d) => (
              <option key={d} value={d}>
                {d} days
              </option>
            ))}
          </select>
        </label>
        <label>
          Date{" "}
          <input
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
            style={input}
          />
        </label>
        {loading && <span className="hint">Loading…</span>}
      </div>

      {error && !loading && (
        <div
          style={{
            color: "var(--error)",
            marginTop: 16,
            whiteSpace: "pre-wrap",
            fontFamily: "monospace",
          }}
        >
          {error}
        </div>
      )}

      <SectionBar
        titles={[
          goals && "Goals",
          (statistics || graphs) && todayTitle,
          statistics && "Due",
          graphs && "Forecast",
          graphs && "Reviews",
          graphs && "True retention",
          history && "Calendar",
          graphs && "Card counts",
          graphs && "Answer buttons",
          graphs && "Hourly breakdown",
          graphs && "Added",
          graphs && "Distributions",
          (statistics || graphs) && "Learning steps",
          (statistics || graphs) && "Scheduler",
          statistics && "Scheduling",
        ]}
      />

      {goals && (
        <>
          <SectionTitle>Goals</SectionTitle>
          <GoalsSection goals={goals} />
        </>
      )}

      {(statistics || graphs) && (
        <div style={{ opacity: loading ? 0.5 : 1 }}>
          <TodaySection
            statistics={statistics}
            graphs={graphs}
            title={todayTitle}
          />

          {statistics && (
            <>
              <SectionTitle>Due</SectionTitle>
              <StateCounts
                counts={statistics.due_count_by_state}
                empty="Nothing due."
              />
            </>
          )}

          {graphs && (
            <>
              <SectionTitle>Forecast</SectionTitle>
              <ForecastSection graphs={graphs} />

              <SectionTitle>Reviews</SectionTitle>
              <ReviewsSection graphs={graphs} />

              <SectionTitle>True retention</SectionTitle>
              <RetentionSection graphs={graphs} />
            </>
          )}

          {history && (
            <>
              <SectionTitle>Calendar</SectionTitle>
              <ReviewHeatmap
                counts={history.review_count_by_date}
                cramCounts={history.cram_count_by_date}
                today={history.today}
              />
            </>
          )}

          {graphs && (
            <>
              <SectionTitle>Card counts</SectionTitle>
              <CardCountsSection graphs={graphs} />

              <SectionTitle>Answer buttons</SectionTitle>
              <AnswerButtonsSection graphs={graphs} />

              <SectionTitle>Hourly breakdown</SectionTitle>
              <HourlySection graphs={graphs} />

              <SectionTitle>Added</SectionTitle>
              <AddedSection graphs={graphs} />

              <SectionTitle>Distributions</SectionTitle>
              <DistributionsSection graphs={graphs} />
            </>
          )}

          <SectionTitle>Learning steps</SectionTitle>
          <StepsSection periodDays={view.periodDays} enabled={canSchedule} />

          <SectionTitle>Scheduler</SectionTitle>
          <SchedulerSection enabled={online} onChanged={reload} />

          {statistics && (
            <>
              <SectionTitle>Scheduling</SectionTitle>
              <div style={tiles}>
                <Tile
                  value={statistics.advance_safe_count}
                  label="Safe to advance"
                />
                <Tile
                  value={statistics.postpone_safe_count}
                  label="Safe to postpone"
                />
              </div>
              {isToday ? (
                <>
                  {/* Keyed on the safe count so the prefilled count follows the latest statistics */}
                  <ScheduleForm
                    key={`a${statistics.advance_safe_count}`}
                    kind="Advance"
                    safeCount={statistics.advance_safe_count}
                    disabled={!canSchedule}
                    onDone={(o) => {
                      showOutcome(o);
                      reload();
                    }}
                  />
                  <ScheduleForm
                    key={`p${statistics.postpone_safe_count}`}
                    kind="Postpone"
                    safeCount={statistics.postpone_safe_count}
                    disabled={!canSchedule}
                    onDone={(o) => {
                      showOutcome(o);
                      reload();
                    }}
                  />
                  {!canSchedule && <p style={note}>{OFFLINE_HINT}</p>}
                  {scheduleOutcome && (
                    <ActionResult
                      key={scheduleOutcome.seq}
                      outcome={scheduleOutcome.outcome}
                      onReverted={reload}
                    />
                  )}
                </>
              ) : (
                <p style={note}>
                  Advancing and postponing apply from now, so they are only
                  offered for today.
                </p>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
