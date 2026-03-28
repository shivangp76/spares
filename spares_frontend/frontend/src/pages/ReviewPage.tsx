import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { getSchedulerRatings, getStatistics, postReview, submitAction } from '../api/client';
import CardRenderer from '../components/CardRenderer';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import type { GetReviewCardResponse, Rating, StatisticsResponse } from '../types/spares';

type Phase = 'landing' | 'loading' | 'front' | 'back' | 'done' | 'error';

const STATE_LABELS: Record<number, string> = {
  0: 'New',
  1: 'Learning',
  2: 'Review',
  3: 'Relearning',
};

function msToSeconds(ms: number): number {
  return Math.max(0, Math.floor(ms / 1000));
}

function backPath(card: GetReviewCardResponse): string {
  const b = card.card_back_rendered_path;
  return 'CardBack' in b ? b.CardBack : b.Note;
}

function backRawPath(card: GetReviewCardResponse): string {
  const b = card.card_back_raw_path;
  return 'CardBack' in b ? b.CardBack : b.Note;
}

export default function ReviewPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();

  const [phase, setPhase] = useState<Phase>('loading');
  const [card, setCard] = useState<GetReviewCardResponse | null>(null);
  const [ratings, setRatings] = useState<Rating[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(null);

  const recallStart = useRef(0);
  const recallEnd = useRef(0);

  const loadCard = useCallback(async (filter?: string) => {
    setPhase('loading');
    try {
      const next = await postReview(filter);
      if (!next) { setPhase('done'); return; }
      setCard(next);
      recallStart.current = Date.now();
      setPhase('front');
    } catch (e) {
      setError(String(e));
      setPhase('error');
    }
  }, []);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    getSchedulerRatings(credentials.schedulerName).then(setRatings).catch(console.error);
    getStatistics(credentials.schedulerName)
      .then(setStatistics)
      .catch(console.error)
      .finally(() => setPhase('landing'));
  }, [credentials, navigate]);

  // Keyboard shortcuts — re-bind on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (phase === 'front' && e.code === 'Space') {
        e.preventDefault();
        showAnswer();
      }
      if (phase === 'back') {
        const idx = parseInt(e.key, 10) - 1;
        if (idx >= 0 && idx < ratings.length) rate(ratings[idx].id);
      }
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });

  function showAnswer() {
    recallEnd.current = Date.now();
    setPhase('back');
  }

  async function rate(ratingId: number) {
    if (!card || !credentials) return;
    const rateEnd = Date.now();
    try {
      await submitAction({
        scheduler_name: credentials.schedulerName,
        action: {
          Rate: {
            card_id: card.card_id,
            rating: ratingId,
            recall_duration: msToSeconds(recallEnd.current - recallStart.current),
            rate_duration: msToSeconds(rateEnd - recallEnd.current),
            tag_id: null,
          },
        },
      });
      loadCard(searchQuery || undefined);
    } catch (e) {
      setError(String(e));
      setPhase('error');
    }
  }

  const cardCounts = card
    ? Object.entries(card.cards_left_by_state)
        .filter(([, count]) => count > 0)
        .map(([stateId, count]) => `${STATE_LABELS[Number(stateId)] ?? stateId}: ${count}`)
        .join(' · ')
    : '';

  return (
    <div style={{ maxWidth: 800, margin: '0 auto', padding: 24 }}>
      <Navbar
        onLogout={logout}
        extra={cardCounts ? <span style={{ fontSize: 13, color: '#666' }}>{cardCounts}</span> : undefined}
      />

      {phase === 'landing' && (
        <div style={{ marginTop: 48 }}>
          {statistics && (
            <div style={{ display: 'flex', gap: 16, marginBottom: 32, flexWrap: 'wrap' }}>
              {Object.entries(statistics.due_count_by_state)
                .filter(([, count]) => count > 0)
                .map(([stateId, count]) => (
                  <div key={stateId} style={{ border: '1px solid #ddd', borderRadius: 6, padding: '12px 20px', textAlign: 'center' }}>
                    <div style={{ fontSize: 24, fontWeight: 600 }}>{count}</div>
                    <div style={{ fontSize: 13, color: '#666' }}>{STATE_LABELS[Number(stateId)] ?? stateId}</div>
                  </div>
                ))}
              {Object.values(statistics.due_count_by_state).every(c => c === 0) && (
                <p style={{ color: '#555' }}>Nothing due to review.</p>
              )}
            </div>
          )}
          <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
            <input
              type="text"
              placeholder="Search / filter cards…"
              value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') loadCard(searchQuery || undefined); }}
              style={{ flex: 1, padding: '8px 12px', fontSize: 14, border: '1px solid #ccc', borderRadius: 4 }}
            />
            <button
              onClick={() => loadCard(searchQuery || undefined)}
              style={{ padding: '8px 24px', fontSize: 14 }}
            >
              Start Review
            </button>
          </div>
        </div>
      )}

      {phase === 'loading' && <div>Loading…</div>}

      {phase === 'done' && (
        <div style={{ textAlign: 'center', marginTop: 80, color: '#555' }}>
          <p>Nothing left to review.</p>
          <Link to="/notes">View notes</Link>
        </div>
      )}

      {phase === 'error' && (
        <div style={{ color: 'red' }}>
          Error: {error}
          <button onClick={() => loadCard(searchQuery || undefined)} style={{ marginLeft: 12 }}>Retry</button>
        </div>
      )}

      {(phase === 'front' || phase === 'back') && card && (
        <>
          <div style={{ marginBottom: 8, fontSize: 13, color: '#888' }}>
            Card {card.card_order} · {card.parser_name}
          </div>

          <div style={{ border: '1px solid #ddd', borderRadius: 4, padding: 16, marginBottom: 16 }}>
            <CardRenderer path={card.parser_name.includes('typst') ? card.card_front_raw_path : card.card_front_rendered_path} parserName={card.parser_name} />
          </div>

          {phase === 'front' && (
            <button onClick={showAnswer} style={{ width: '100%', padding: 12 }}>
              Show Answer <span style={{ color: '#999', fontSize: 12 }}>(Space)</span>
            </button>
          )}

          {phase === 'back' && (
            <>
              <div style={{ border: '1px solid #ddd', borderRadius: 4, padding: 16, marginBottom: 16, background: '#fafafa' }}>
                <CardRenderer path={card.parser_name.includes('typst') ? backRawPath(card) : backPath(card)} parserName={card.parser_name} />
              </div>
              <div style={{ display: 'flex', gap: 8 }}>
                {ratings.map((r, i) => (
                  <button key={r.id} onClick={() => rate(r.id)} style={{ flex: 1, padding: 12 }}>
                    {r.description}
                    <span style={{ display: 'block', color: '#999', fontSize: 12 }}>({i + 1})</span>
                  </button>
                ))}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
