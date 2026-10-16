import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { formatValue as defaultFormat, mergeColumns, niceMax, totals, type Series } from './chartData';

const AXIS_WIDTH = 40;
const LABEL_HEIGHT = 18;
const LINE_HEIGHT = 56;
/** The narrowest a bar may be before days are merged */
const MIN_BAR = 3;
const tick: React.CSSProperties = { fontSize: 10, fill: 'var(--text-muted)' };
const toggle: React.CSSProperties = { padding: '2px 10px', fontSize: 12 };

/** A line drawn in a panel of its own under the bars, on its own axis, sharing the bars' columns. */
export interface ChartLine {
  label: string;
  color: string;
  /** `null` where there is nothing to show, e.g. % correct in an hour without reviews: the line breaks there. */
  values: (number | null)[];
  format?: (value: number) => string;
  /** The axis' top; the line's largest value rounded up if not given. */
  max?: number;
  /** When columns are merged: the last value of each (a running total) rather than their mean. */
  merge?: 'last' | 'mean';
}

interface BarChartProps {
  /** Names the chart for screen readers and the table. */
  title: string;
  /** One per column, e.g. its date. */
  labels: string[];
  /** Stacked from the bottom up. */
  series: Series[];
  line?: ChartLine;
  formatValue?: (value: number) => string;
  /** The heading of the table's first column, e.g. "Date". */
  labelHeading?: string;
  height?: number;
  /** Merge columns to fit the width. Off for charts whose columns mean something apart, e.g. hours. */
  mergeToFit?: boolean;
}

/** A rectangle with its top corners rounded by `r`, so a stack's end is rounded and its base square. */
function roundedTop(x: number, y: number, w: number, h: number, r: number): string {
  const radius = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + radius}Q${x},${y} ${x + radius},${y}H${x + w - radius}Q${x + w},${y} ${x + w},${y + radius}V${y + h}Z`;
}

function mergeLine(line: ChartLine, size: number): (number | null)[] {
  if (size <= 1) return line.values;
  const merged: (number | null)[] = [];
  for (let i = 0; i < line.values.length; i += size) {
    const chunk = line.values.slice(i, i + size).filter((v): v is number => v !== null);
    if (chunk.length === 0) merged.push(null);
    else merged.push(line.merge === 'last' ? chunk[chunk.length - 1] : chunk.reduce((a, b) => a + b, 0) / chunk.length);
  }
  return merged;
}

/** The runs of consecutive values that are not `null`, as polyline points. */
function lineSegments(values: (number | null)[], x: (i: number) => number, y: (v: number) => number): string[] {
  const segments: string[] = [];
  let points: string[] = [];
  values.forEach((value, i) => {
    if (value === null) {
      if (points.length) segments.push(points.join(' '));
      points = [];
    } else {
      points.push(`${x(i)},${y(value)}`);
    }
  });
  if (points.length) segments.push(points.join(' '));
  return segments;
}

/**
 * A stacked bar chart drawn as SVG, as wide as its container. Hovering or tapping a column shows
 * its values, and the chart can be shown as a table instead, to read or copy the numbers.
 */
export default function BarChart({ title, labels: allLabels, series: allSeries, line, formatValue = defaultFormat, labelHeading = 'Date', height = 160, mergeToFit = true }: BarChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [active, setActive] = useState<number | null>(null);
  const [asTable, setAsTable] = useState(false);

  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setWidth(el.clientWidth));
    observer.observe(el);
    setWidth(el.clientWidth);
    return () => observer.disconnect();
  }, [asTable]);

  const innerWidth = Math.max(0, width - AXIS_WIDTH);
  const maxColumns = mergeToFit ? Math.max(1, Math.floor(innerWidth / MIN_BAR)) : allLabels.length;
  const { labels, series } = useMemo(() => mergeColumns(allLabels, allSeries, maxColumns), [allLabels, allSeries, maxColumns]);
  const mergeSize = labels.length < allLabels.length ? Math.ceil(allLabels.length / maxColumns) : 1;
  const lineValues = useMemo(() => (line ? mergeLine(line, mergeSize) : []), [line, mergeSize]);
  const stackTotals = useMemo(() => totals(series, labels.length), [series, labels.length]);
  const max = niceMax(Math.max(0, ...stackTotals));
  const lineMax = line ? (line.max ?? niceMax(Math.max(0, ...lineValues.filter((v): v is number => v !== null)))) : 1;
  const formatLine = (value: number | null | undefined) => (value === null || value === undefined ? '–' : (line?.format ?? defaultFormat)(value));

  const n = Math.max(1, labels.length);
  const band = innerWidth / n;
  const gap = band > 6 ? 2 : band > 3 ? 1 : 0;
  const barWidth = Math.max(1, band - gap);
  const y = (value: number) => height - (value / max) * height;
  const lineY = (value: number) => LINE_HEIGHT - 4 - (value / lineMax) * (LINE_HEIGHT - 8);

  // At most three labels along the bottom, so they never collide
  const xLabels = labels.length === 0 ? [] : labels.length < 3 ? labels.map((_, i) => i) : [0, Math.floor((labels.length - 1) / 2), labels.length - 1];

  const header = (
    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8, flexWrap: 'wrap', marginBottom: 6 }}>
      {series.length > 1 ? (
        <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap', fontSize: 12, color: 'var(--text-secondary)' }}>
          {series.map(s => (
            <span key={s.key} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
              <span aria-hidden style={{ width: 10, height: 10, borderRadius: 2, background: s.color, display: 'inline-block' }} />
              {s.label}
            </span>
          ))}
        </div>
      ) : <span />}
      <button onClick={() => setAsTable(t => !t)} style={toggle} aria-pressed={asTable} title={asTable ? 'Show as a chart' : 'Show as a table'}>
        {asTable ? 'Chart' : 'Table'}
      </button>
    </div>
  );

  if (asTable) {
    return (
      <div>
        {header}
        <div className="table-scroll" style={{ maxHeight: 320, overflowY: 'auto' }}>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13, fontVariantNumeric: 'tabular-nums' }}>
            <caption style={{ position: 'absolute', left: -9999 }}>{title}</caption>
            <thead>
              <tr>
                <th style={cellHead}>{labelHeading}</th>
                {allSeries.map(s => <th key={s.key} style={{ ...cellHead, textAlign: 'right' }}>{s.label}</th>)}
                {allSeries.length > 1 && <th style={{ ...cellHead, textAlign: 'right' }}>Total</th>}
                {line && <th style={{ ...cellHead, textAlign: 'right' }}>{line.label}</th>}
              </tr>
            </thead>
            <tbody>
              {allLabels.map((label, i) => (
                <tr key={label}>
                  <td style={cell}>{label}</td>
                  {allSeries.map(s => <td key={s.key} style={{ ...cell, textAlign: 'right' }}>{formatValue(s.values[i] ?? 0)}</td>)}
                  {allSeries.length > 1 && <td style={{ ...cell, textAlign: 'right' }}>{formatValue(allSeries.reduce((sum, s) => sum + (s.values[i] ?? 0), 0))}</td>}
                  {line && <td style={{ ...cell, textAlign: 'right' }}>{formatLine(line.values[i])}</td>}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    );
  }

  const tooltipLeft = active === null ? 0 : Math.min(Math.max(AXIS_WIDTH + (active + 0.5) * band - 90, 0), Math.max(0, width - 180));

  return (
    <div>
      {header}
      <div ref={containerRef} style={{ position: 'relative', width: '100%' }} onMouseLeave={() => setActive(null)}>
        {width > 0 && (
          <svg width={width} height={height + LABEL_HEIGHT + (line ? LINE_HEIGHT : 0)} role="img" aria-label={title} style={{ display: 'block', overflow: 'visible' }}>
            {/* Recessive grid: the baseline, the middle and the top */}
            {[0, max / 2, max].map(value => (
              <g key={value}>
                <line x1={AXIS_WIDTH} x2={width} y1={y(value)} y2={y(value)} stroke="var(--border-subtle)" strokeWidth={1} />
                <text x={AXIS_WIDTH - 6} y={y(value) + 3} textAnchor="end" style={tick}>{formatValue(value)}</text>
              </g>
            ))}
            {labels.map((label, i) => {
              let base = 0;
              const top = stackTotals[i];
              const x = AXIS_WIDTH + i * band + gap / 2;
              return (
                <g key={label} opacity={active === null || active === i ? 1 : 0.55}>
                  {series.map(s => {
                    const value = s.values[i] ?? 0;
                    if (value <= 0) return null;
                    const y0 = y(base + value);
                    const h = Math.max(1, y(base) - y0);
                    base += value;
                    const isTop = base >= top;
                    // A gap of the page's colour between stacked segments
                    return isTop
                      ? <path key={s.key} d={roundedTop(x, y0, barWidth, h, 3)} fill={s.color} stroke="var(--bg)" strokeWidth={barWidth > 4 ? 1 : 0} />
                      : <rect key={s.key} x={x} y={y0} width={barWidth} height={h} fill={s.color} stroke="var(--bg)" strokeWidth={barWidth > 4 ? 1 : 0} />;
                  })}
                </g>
              );
            })}
            {xLabels.map(i => (
              <text
                key={i}
                x={AXIS_WIDTH + (i + 0.5) * band}
                y={height + 13}
                textAnchor={i === 0 && labels.length > 2 ? 'start' : i === labels.length - 1 && labels.length > 2 ? 'end' : 'middle'}
                style={tick}
              >
                {labels[i].split(' – ')[0]}
              </text>
            ))}
            {line && (
              <g transform={`translate(0, ${height + LABEL_HEIGHT})`}>
                <text x={AXIS_WIDTH - 6} y={lineY(lineMax) + 3} textAnchor="end" style={tick}>{formatLine(lineMax)}</text>
                <text x={AXIS_WIDTH - 6} y={lineY(0) + 3} textAnchor="end" style={tick}>{formatLine(0)}</text>
                <line x1={AXIS_WIDTH} x2={width} y1={lineY(0)} y2={lineY(0)} stroke="var(--border-subtle)" strokeWidth={1} />
                <text x={width} y={8} textAnchor="end" style={tick}>{line.label}</text>
                {lineSegments(lineValues, i => AXIS_WIDTH + (i + 0.5) * band, lineY).map(points => (
                  points.includes(' ')
                    ? <polyline key={points} fill="none" stroke={line.color} strokeWidth={2} strokeLinejoin="round" points={points} />
                    // A lone value gets a dot, as it has no neighbour to draw a line to
                    : <circle key={points} cx={Number(points.split(',')[0])} cy={Number(points.split(',')[1])} r={2.5} fill={line.color} />
                ))}
                {active !== null && typeof lineValues[active] === 'number' && (
                  <circle cx={AXIS_WIDTH + (active + 0.5) * band} cy={lineY(lineValues[active] as number)} r={4} fill={line.color} stroke="var(--bg)" strokeWidth={2} />
                )}
              </g>
            )}
            {/* Hit targets the full height of each column, larger than its bar */}
            {labels.map((label, i) => (
              <rect
                key={label}
                x={AXIS_WIDTH + i * band}
                y={0}
                width={band}
                height={height + LABEL_HEIGHT + (line ? LINE_HEIGHT : 0)}
                fill="transparent"
                onMouseEnter={() => setActive(i)}
                onClick={() => setActive(a => (a === i ? null : i))}
              />
            ))}
          </svg>
        )}
        {active !== null && labels[active] !== undefined && (
          <div role="status" style={{ position: 'absolute', top: 0, left: tooltipLeft, width: 180, pointerEvents: 'none', background: 'var(--bg)', border: '1px solid var(--border-strong)', borderRadius: 6, padding: '6px 8px', fontSize: 12, boxShadow: '0 2px 6px rgb(0 0 0 / 0.15)', fontVariantNumeric: 'tabular-nums' }}>
            <div style={{ fontWeight: 600, marginBottom: 4 }}>{labels[active]}</div>
            {series.map(s => (
              <div key={s.key} style={{ display: 'flex', justifyContent: 'space-between', gap: 8, color: 'var(--text-secondary)' }}>
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                  <span aria-hidden style={{ width: 8, height: 8, borderRadius: 2, background: s.color, display: 'inline-block' }} />
                  {s.label}
                </span>
                <span style={{ color: 'var(--text)' }}>{formatValue(s.values[active] ?? 0)}</span>
              </div>
            ))}
            {series.length > 1 && (
              <div style={{ display: 'flex', justifyContent: 'space-between', borderTop: '1px solid var(--border-subtle)', marginTop: 4, paddingTop: 4 }}>
                <span>Total</span><span>{formatValue(stackTotals[active])}</span>
              </div>
            )}
            {line && (
              <div style={{ display: 'flex', justifyContent: 'space-between', color: 'var(--text-secondary)' }}>
                <span>{line.label}</span><span style={{ color: 'var(--text)' }}>{formatLine(lineValues[active])}</span>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

const cellHead: React.CSSProperties = { textAlign: 'left', padding: '4px 8px', borderBottom: '1px solid var(--border-strong)', position: 'sticky', top: 0, background: 'var(--bg)' };
const cell: React.CSSProperties = { padding: '4px 8px', borderBottom: '1px solid var(--border-subtle)' };
