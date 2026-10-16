import { useMemo } from 'react';
import type { Histogram as HistogramData } from '../../types/spares';
import BarChart from './BarChart';
import { formatValue } from './chartData';

/** Cards in the server's buckets, labeled by their edges, e.g. `10–20 days`. */
export default function Histogram({ title, data, unit = '', format = formatValue, color = 'var(--chart-bar)' }: {
  title: string;
  data: HistogramData;
  unit?: string;
  /** Formats an edge, e.g. as a percentage for retrievability. */
  format?: (value: number) => string;
  color?: string;
}) {
  const { labels, series } = useMemo(() => ({
    labels: data.counts.map((_, i) => `${format(data.edges[i])}–${format(data.edges[i + 1])}${unit}`),
    series: [{ key: 'cards', label: 'Cards', color, values: data.counts }],
  }), [data, unit, format, color]);
  if (data.counts.length === 0) return <p style={{ color: 'var(--text-secondary)', fontSize: 14 }}>No cards.</p>;
  return <BarChart title={title} labels={labels} series={series} labelHeading={unit.trim() ? `Range (${unit.trim()})` : 'Range'} height={120} mergeToFit={false} />;
}
