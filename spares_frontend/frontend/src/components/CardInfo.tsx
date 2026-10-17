import { useEffect, useState } from 'react';
import { getCardReviews, getSchedulerRatings } from '../api/client';
import { STATE_LABELS, type CardReviewEntry, type Rating } from '../types/spares';
import { useFieldAbout } from '../hooks/useFieldAbout';
import { errorMessage, formatDuration, formatInterval, td, th } from '../utils';

const metaLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', fontWeight: 600 };
const REVIEW_STATE = 2;

interface Props {
  cardId: number;
  /** Bump to refetch, e.g. after the card was rated, forgotten or an action on it was undone. */
  version?: number;
}

/** A card's review history and totals, like Anki's Card Info. */
export default function CardInfo({ cardId, version = 0 }: Props) {
  const [loaded, setLoaded] = useState<{ cardId: number; version: number; entries: CardReviewEntry[] } | { cardId: number; version: number; error: string } | null>(null);
  // Rating ids are relative to the scheduler that recorded them
  const [ratingsByScheduler, setRatingsByScheduler] = useState<Record<string, Rating[]>>({});
  const about = useFieldAbout();

  useEffect(() => {
    let cancelled = false;
    getCardReviews(cardId).then(
      entries => { if (!cancelled) setLoaded({ cardId, version, entries }); },
      (e: unknown) => { if (!cancelled) setLoaded({ cardId, version, error: errorMessage(e) }); },
    );
    return () => { cancelled = true; };
  }, [cardId, version]);

  const current = loaded && loaded.cardId === cardId ? loaded : null;
  const entries = current && 'entries' in current ? current.entries : null;

  useEffect(() => {
    if (!entries) return;
    let cancelled = false;
    for (const name of new Set(entries.map(e => e.scheduler_name))) {
      getSchedulerRatings(name).then(
        ratings => { if (!cancelled) setRatingsByScheduler(prev => ({ ...prev, [name]: ratings })); },
        console.error,
      );
    }
    return () => { cancelled = true; };
  }, [entries]);

  if (!current) return <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading…</div>;
  if ('error' in current) return <div style={{ color: 'var(--error)', fontSize: 13 }}>{current.error}</div>;
  if (current.entries.length === 0) return <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>This card has not been reviewed yet.</div>;

  const ratingLabel = (entry: CardReviewEntry) =>
    ratingsByScheduler[entry.scheduler_name]?.find(r => r.id === entry.rating)?.description;
  const reviews = current.entries.filter(e => e.kind === 'Review');
  // Anki's meaning: forgetting a card that had graduated to review
  const lapses = reviews.filter(e => e.previous_state === REVIEW_STATE && ratingLabel(e) === 'Again').length;
  const timeTaken = (e: CardReviewEntry) => (e.recall_duration ?? 0) + (e.rate_duration ?? 0);
  const totalTime = reviews.reduce((sum, e) => sum + timeTaken(e), 0);
  // Ratings in filtered tags that do not reschedule, counted apart since they left the schedule alone
  const crams = current.entries.filter(e => e.kind === 'Cram');
  const cramTime = crams.reduce((sum, e) => sum + timeTaken(e), 0);
  const date = (iso: string) => new Date(iso).toLocaleString();

  return (
    <div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))', gap: '8px 16px', fontSize: 13, marginBottom: 12 }}>
        {reviews.length > 0 && <div><div style={metaLabel}>First Review</div>{date(reviews[0].reviewed_at)}</div>}
        {reviews.length > 0 && <div><div style={metaLabel}>Latest Review</div>{date(reviews[reviews.length - 1].reviewed_at)}</div>}
        <div><div style={metaLabel}>Reviews</div>{reviews.length}</div>
        <div><div style={metaLabel}>Lapses</div>{lapses}</div>
        {reviews.length > 0 && <div><div style={metaLabel}>Average Time</div>{formatDuration(Math.round(totalTime / reviews.length))}</div>}
        <div><div style={metaLabel}>Total Time</div>{formatDuration(totalTime)}</div>
        {crams.length > 0 && <div><div style={metaLabel}>Crams</div>{crams.length}</div>}
        {crams.length > 0 && <div><div style={metaLabel}>Cram Time</div>{formatDuration(cramTime)}</div>}
      </div>

      <div style={{ overflowX: 'auto' }}>
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
          <thead>
            <tr>
              <th style={th}>Date</th>
              <th style={th}>Type</th>
              <th style={th}>Rating</th>
              <th style={th} className="explained" title={about('c.interval')}>Interval</th>
              <th style={th}>Time</th>
              <th style={th} className="explained" title={about('c.stability')}>Stability</th>
              <th style={th} className="explained" title={about('c.difficulty')}>Difficulty</th>
            </tr>
          </thead>
          <tbody>
            {/* Newest first, like Anki */}
            {[...current.entries].reverse().map(e => (
              <tr key={e.id}>
                <td style={td}>{date(e.reviewed_at)}</td>
                <td style={td}>{e.kind === 'Forget' ? 'Forget' : e.kind === 'Cram' ? 'Cram' : e.tag_id !== null ? 'Filtered' : STATE_LABELS[e.previous_state] ?? e.previous_state}</td>
                <td style={td}>{e.rating === null ? '—' : ratingLabel(e) ?? e.rating}</td>
                <td style={td}>{e.scheduled_time === null ? '—' : formatInterval(e.scheduled_time)}</td>
                <td style={td}>{e.kind === 'Forget' ? '—' : formatDuration(timeTaken(e))}</td>
                <td style={td}>{e.stability === null ? '—' : e.stability.toFixed(2)}</td>
                <td style={td}>{e.difficulty === null ? '—' : e.difficulty.toFixed(2)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 6 }}>
        Stability and difficulty are replayed with the scheduler's current parameters.
      </div>
    </div>
  );
}
