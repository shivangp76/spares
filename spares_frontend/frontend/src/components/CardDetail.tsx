import { useEffect, useState } from 'react';
import { forgetCard, getCard, getNote, getReviewCardById, updateCards } from '../api/client';
import { STATE_LABELS, type CardResponse, type GetReviewCardResponse, type NoteResponse } from '../types/spares';
import { backPath, sectionLabel } from '../utils';
import ActionResult, { type ActionOutcome } from './ActionResult';
import CardRenderer from './CardRenderer';
import NoteDetail from './NoteDetail';

const actionButton: React.CSSProperties = { padding: '6px 10px', fontSize: 13 };
const metaLabel: React.CSSProperties = { fontSize: 12, color: 'var(--text-muted)', fontWeight: 600 };
const renderBox: React.CSSProperties = { border: '1px solid var(--border)', borderRadius: 4, padding: 4, marginBottom: 12, background: 'var(--bg)' };

interface Props {
  card: CardResponse;
  index: number;
  total: number;
  onGoTo: (index: number) => void;
  onClose: () => void;
  /** Called with the card's new state after an action on it, or after undoing one. */
  onCardChanged: (card: CardResponse) => void;
}

/** One card of a list being stepped through, like `spares card view`. */
export default function CardDetail({ card, index, total, onGoTo, onClose, onCardChanged }: Props) {
  // Bumped after the note is saved so the rendered files are refetched
  const [version, setVersion] = useState(0);
  const [review, setReview] = useState<{ cardId: number; version: number; result: GetReviewCardResponse | null } | { cardId: number; version: number; error: string } | null>(null);
  const [panelNote, setPanelNote] = useState<NoteResponse | null>(null);
  const [goTo, setGoTo] = useState('');
  const [outcome, setOutcome] = useState<{ seq: number; outcome: ActionOutcome } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    getReviewCardById(card.id).then(
      result => { if (!cancelled) setReview({ cardId: card.id, version, result }); },
      (e: unknown) => { if (!cancelled) setReview({ cardId: card.id, version, error: String(e) }); },
    );
    return () => { cancelled = true; };
  }, [card.id, version]);

  const loaded = review && review.cardId === card.id ? review : null;

  async function run(action: () => Promise<ActionOutcome>) {
    setError(null);
    try {
      const result = await action();
      setOutcome(prev => ({ seq: (prev?.seq ?? 0) + 1, outcome: result }));
    } catch (e) {
      setError(String(e));
    }
  }

  function toggleSuspend() {
    const suspended = card.special_state === 'Suspended';
    run(async () => {
      const res = await updateCards({ selector: { Ids: [card.id] }, special_state: suspended ? null : 'Suspended' });
      onCardChanged(res.cards[0]);
      return { message: suspended ? 'Card unsuspended.' : 'Card suspended.', eventIds: res.event_id === null ? [] : [res.event_id] };
    });
  }

  function forget() {
    if (!window.confirm(`Forget card ${card.id}? Its scheduling is reset; review logs are kept.`)) return;
    run(async () => {
      const res = await forgetCard(card.id);
      onCardChanged(res.card);
      return { message: 'Card forgotten (scheduling reset).', eventIds: res.event_id === null ? [] : [res.event_id] };
    });
  }

  async function refreshCard() {
    try {
      onCardChanged(await getCard(card.id));
    } catch (e) {
      setError(String(e));
    }
  }

  async function openNote(noteId: number) {
    try {
      setPanelNote(await getNote(noteId));
    } catch (e) {
      setError(String(e));
    }
  }

  function submitGoTo() {
    const n = Number(goTo);
    if (!Number.isInteger(n) || n < 1 || n > total) {
      setError(`Invalid item number. Please enter a number between 1 and ${total}.`);
      return;
    }
    setError(null);
    setGoTo('');
    onGoTo(n - 1);
  }

  const reviewCard = loaded && 'result' in loaded ? loaded.result : null;

  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: 20, position: 'relative', backgroundColor: 'var(--surface)' }}>
      <button
        onClick={onClose}
        style={{ position: 'absolute', top: 12, right: 12, background: 'none', border: 'none', fontSize: 18, cursor: 'pointer', color: 'var(--text-secondary)', lineHeight: 1 }}
        aria-label="Close detail"
        className="tap-area"
      >×</button>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 12, paddingRight: 24 }}>
        {/* Wraps around at either end, as in the CLI */}
        <button onClick={() => onGoTo((index + total - 1) % total)} disabled={total < 2} className="touch-target-small" style={actionButton}>Previous</button>
        <span style={{ fontSize: 14 }}>Card {index + 1} of {total}</span>
        <button onClick={() => onGoTo((index + 1) % total)} disabled={total < 2} className="touch-target-small" style={actionButton}>Next</button>
        <input
          type="number"
          min={1}
          max={total}
          value={goTo}
          onChange={e => setGoTo(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') submitGoTo(); }}
          placeholder="Go to…"
          aria-label="Go to item number"
          style={{ width: 80, padding: '5px 8px', fontSize: 13 }}
        />
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))', gap: '8px 16px', fontSize: 13, marginBottom: 16 }}>
        <div><div style={metaLabel}>Card Id</div>{card.id}</div>
        <div><div style={metaLabel}>Note Id</div>{card.note_id}</div>
        <div><div style={metaLabel}>Parser</div>{reviewCard?.parser_name ?? '…'}</div>
        <div><div style={metaLabel}>Order</div>{card.order}</div>
        <div><div style={metaLabel}>State</div>{STATE_LABELS[card.state] ?? card.state}</div>
        <div><div style={metaLabel}>Due</div>{new Date(card.due).toLocaleString()}</div>
        <div><div style={metaLabel}>Desired Retention</div>{card.desired_retention}</div>
        <div><div style={metaLabel}>Stability</div>{card.stability.toFixed(2)}</div>
        <div><div style={metaLabel}>Difficulty</div>{card.difficulty.toFixed(2)}</div>
        {card.special_state && <div><div style={metaLabel}>Special</div>{card.special_state}</div>}
      </div>

      {!loaded && <div style={{ fontSize: 13, color: 'var(--text-muted)', marginBottom: 12 }}>Loading…</div>}
      {loaded && 'error' in loaded && <div style={{ color: 'var(--error)', fontSize: 13, marginBottom: 12 }}>{loaded.error}</div>}
      {loaded && 'result' in loaded && loaded.result === null && <div style={{ color: 'var(--text-muted)', fontSize: 13, marginBottom: 12 }}>Card no longer exists.</div>}
      {reviewCard && (reviewCard.cli ? (
        <div style={renderBox}>
          <pre style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{reviewCard.cli.surrounding}</pre>
          <p style={{ color: 'var(--warning)', fontSize: 13, marginBottom: 0 }}>
            This card is reviewed by running <code>{reviewCard.cli.exec}</code>, which only the CLI can do.
          </p>
        </div>
      ) : (
        <>
          <div style={sectionLabel}>Front</div>
          <div style={renderBox}>
            <CardRenderer path={reviewCard.card_front_rendered_path} parserName={reviewCard.parser_name} source={reviewCard.browser_sources?.card_front} version={loaded?.version} />
          </div>
          <div style={sectionLabel}>Back</div>
          <div style={renderBox}>
            <CardRenderer path={backPath(reviewCard)} parserName={reviewCard.parser_name} source={reviewCard.browser_sources?.card_back} version={loaded?.version} />
          </div>
        </>
      ))}

      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', marginBottom: 8 }}>
        <button onClick={() => (panelNote?.id === card.note_id ? setPanelNote(null) : openNote(card.note_id))} style={actionButton}>
          {panelNote?.id === card.note_id ? 'Close Note' : 'Edit Note'}
        </button>
        <button onClick={toggleSuspend} style={actionButton}>{card.special_state === 'Suspended' ? 'Unsuspend Card' : 'Suspend Card'}</button>
        <button onClick={forget} style={actionButton}>Forget Card</button>
      </div>

      {reviewCard && reviewCard.linked_notes.length > 0 && (
        <details style={{ marginBottom: 8 }}>
          <summary style={{ cursor: 'pointer', fontSize: 13 }}>Open Linked Notes ({reviewCard.linked_notes.length})</summary>
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 8 }}>
            {reviewCard.linked_notes.map(ln => (
              <button key={`${ln.searched_keyword}-${ln.note_id}`} onClick={() => openNote(ln.note_id)} style={actionButton}>
                {ln.searched_keyword} ({ln.note_id})
              </button>
            ))}
          </div>
        </details>
      )}

      {error && <div style={{ color: 'var(--error)', fontSize: 13 }}>Error: {error}</div>}
      {outcome && <ActionResult key={outcome.seq} outcome={outcome.outcome} onUndone={refreshCard} />}

      {panelNote && (
        <div style={{ marginTop: 16 }}>
          <NoteDetail
            key={panelNote.id}
            note={panelNote}
            onClose={() => setPanelNote(null)}
            onNoteUpdated={updated => {
              setPanelNote(updated);
              // The note was re-rendered, which may have changed this card or its files
              if (updated.id === card.note_id) {
                setVersion(v => v + 1);
                refreshCard();
              }
            }}
            onOpenNote={openNote}
          />
        </div>
      )}
    </div>
  );
}
