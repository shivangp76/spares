import { useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { ApiError, getQueuePositions, getTagByName, isNetworkError } from '../api/client';
import SearchInput, { type SearchInputError } from './SearchInput';
import { STATE_LABELS, type NotShownReason, type QueueCard, type QueuePositionResponse, type ReviewFilter } from '../types/spares';
import { errorMessage, formatNoteId, td, th } from '../utils';

/** The start of a note's text, cut to the column like the notes list's Data column. */
const dataTd: React.CSSProperties = { ...td, maxWidth: 300, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };

/** How many cards of the queue a preview lists, as `spares card queue` does without `--match`. */
const PREVIEW_LIMIT = 50;

/** How long typing pauses before the positions are looked up, as for the next card of the filter. */
const LOOKUP_DELAY_MS = 500;

function reasonLabel(reason: NotShownReason): string {
  if (reason === 'Suspended') return 'Suspended';
  if (reason === 'Buried') return 'Buried';
  if (reason === 'NotDue') return 'Not due';
  if (reason === 'WaitingOnLinkedNote') return 'Waits on a linked note';
  const [kind, { preset }] = 'NewLimit' in reason ? ['New', reason.NewLimit] : ['Review', reason.ReviewLimit];
  return preset === null ? `${kind} card limit` : `${kind} card limit of ${preset}`;
}

const time = (iso: string) => new Date(iso).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });

interface Row {
  key: string;
  first: string;
  card: QueueCard;
  muted?: boolean;
}

/**
 * The cards of a queue position lookup: those in the queue by position, then those held back by
 * the learn ahead limit and those the review would not show today, with why. Clicking a row opens
 * its card with `onOpen`.
 */
export function QueuePositionList({ response, onOpen, lookingUp, currentCardId }: {
  response: QueuePositionResponse;
  onOpen: (card: QueueCard) => void;
  /** Whether a query picked the cards, rather than the whole queue being listed. */
  lookingUp: boolean;
  /** The card a session shows now, which is marked. */
  currentCardId?: number;
}) {
  const sections: { heading: string | null; firstHeading: string; rows: Row[] }[] = [
    {
      heading: null,
      firstHeading: 'Position',
      rows: response.positions.map(p => ({ key: `p${p.card_id}`, first: String(p.position), card: p })),
    },
    {
      heading: 'Held back by the learn ahead limit, shown once nothing else is left',
      firstHeading: 'Shown from',
      rows: response.held_back.map(c => ({ key: `h${c.card_id}`, first: time(c.shown_from), card: c, muted: true })),
    },
    {
      heading: 'Not shown today',
      firstHeading: 'Why',
      rows: response.not_shown.map(c => ({ key: `n${c.card_id}`, first: reasonLabel(c.reason), card: c, muted: true })),
    },
  ];
  const shown = response.positions.length;
  const listed = lookingUp ? response.matching : response.total;
  const summary = `${response.total} card${response.total === 1 ? '' : 's'} in this review`
    + (lookingUp ? `. ${response.matching} match${response.matching === 1 ? 'es' : ''}` : '')
    + (shown < listed ? `. The first ${shown}:` : '.');
  return (
    <div style={{ fontSize: 14 }}>
      <style>{`
        .queue-row:hover { background-color: var(--hover); }
        .queue-row-current { background-color: var(--selected) !important; }
      `}</style>
      <div style={{ marginBottom: 8, color: 'var(--text-secondary)' }}>{summary}</div>
      {sections.filter(s => s.rows.length > 0).map(section => (
        <div key={section.firstHeading} style={{ marginBottom: 12 }}>
          {section.heading && <div style={{ fontSize: 13, color: 'var(--text-muted)', margin: '12px 0 4px' }}>{section.heading}</div>}
          <div className="table-scroll">
            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
              <thead>
                <tr>
                  <th style={th}>{section.firstHeading}</th>
                  <th style={th}>Note</th>
                  <th style={th}>Data</th>
                  <th style={th} className="hide-narrow">Card</th>
                  <th style={th}>State</th>
                  <th style={th}>Due</th>
                </tr>
              </thead>
              <tbody>
                {section.rows.map(({ key, first, card, muted }) => (
                  <tr
                    key={key}
                    className={`queue-row${card.card_id === currentCardId ? ' queue-row-current' : ''}`}
                    onClick={() => onOpen(card)}
                    style={{ cursor: 'pointer', color: muted ? 'var(--text-secondary)' : undefined }}
                    title={card.card_id === currentCardId ? 'The card shown now' : undefined}
                  >
                    <td style={{ ...td, whiteSpace: 'nowrap' }}>{first}</td>
                    <td style={td}>{formatNoteId(card.note_id)}</td>
                    <td style={dataTd} className="clamp-narrow" title={card.note_preview}><span>{card.note_preview}</span></td>
                    <td style={td} className="hide-narrow">{card.card_order}</td>
                    <td style={td}>{STATE_LABELS[card.card_state] ?? card.card_state}</td>
                    <td style={{ ...td, whiteSpace: 'nowrap' }}>{time(card.due)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ))}
      {response.not_shown_total > response.not_shown.length && (
        <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>And {response.not_shown_total - response.not_shown.length} more not shown today.</div>
      )}
      {response.not_in_review > 0 && (
        <div style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 4 }}>
          {response.not_in_review} matching card{response.not_in_review === 1 ? ' is' : 's are'} not in this review's filter.
        </div>
      )}
      <div style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 8 }}>Positions change as you rate cards.</div>
    </div>
  );
}

export type FilterMode = 'query' | 'tagName' | 'tagId';

/** The review filter `input` names in `mode`, as Start Review resolves it, or `null` for all due cards. */
async function resolveFilter(mode: FilterMode, input: string): Promise<ReviewFilter | null> {
  if (!input) return null;
  // The server looks a query with `limit` up in today's snapshot of it
  if (mode === 'query') return { Query: input };
  if (mode === 'tagName') return { FilteredTag: { tag_id: (await getTagByName(input)).id } };
  const id = Number(input);
  if (!Number.isInteger(id)) throw new Error(`Invalid tag id: ${input}`);
  return { FilteredTag: { tag_id: id } };
}

/** The review to look in: as typed on the landing view, or as a session resolved it. */
export type QueueFilter = { mode: FilterMode; input: string } | { resolved: ReviewFilter | null };

type Lookup =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'done'; response: QueuePositionResponse; lookingUp: boolean }
  | { kind: 'error'; message: string }
  | { kind: 'offline' };

/**
 * Find in queue, on the review page: the queue of `filter`, from its first card, or, with a query
 * typed in its box, where the cards matching it are in that queue. The query is kept in `?match=`,
 * so the landing view and a session share it.
 *
 * On the landing view, `filter` is the one typed above it. During a session, it is the one the
 * session resolved, such as the snapshot of a query with `limit`, and `refreshKey` changes with each
 * card shown, so the positions follow the session. `currentCardId` marks the card shown now.
 */
export default function FindInQueue({ filter, onOpenNote, onClose, refreshKey, currentCardId, autoFocus, style }: {
  filter: QueueFilter;
  onOpenNote: (noteId: number) => void;
  onClose: () => void;
  refreshKey?: unknown;
  currentCardId?: number;
  autoFocus?: boolean;
  /** Merged into the panel's style. */
  style?: React.CSSProperties;
}) {
  const [searchParams, setSearchParams] = useSearchParams();
  const [match, setMatch] = useState(() => searchParams.get('match') ?? '');
  // The match query that was looked up, which the box and `?match=` follow
  const [submitted, setSubmitted] = useState(() => searchParams.get('match') ?? '');
  const [lookup, setLookup] = useState<Lookup>({ kind: 'loading' });
  const [matchError, setMatchError] = useState<SearchInputError | null>(null);
  const input = useRef<HTMLInputElement>(null);
  // The filter as a string, so a new object for the same filter on each render looks nothing up
  const filterKey = JSON.stringify('resolved' in filter ? filter : { ...filter, input: filter.input.trim() });

  useEffect(() => {
    if (autoFocus) input.current?.focus();
  }, [autoFocus]);

  useEffect(() => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (submitted) next.set('match', submitted);
      else next.delete('match');
      return next;
    }, { replace: true });
  }, [submitted, setSearchParams]);

  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(async () => {
      setLookup(prev => (prev.kind === 'done' ? prev : { kind: 'loading' }));
      try {
        const queueFilter = JSON.parse(filterKey) as QueueFilter;
        const response = await getQueuePositions({
          filter: 'resolved' in queueFilter ? queueFilter.resolved : await resolveFilter(queueFilter.mode, queueFilter.input),
          match_query: submitted || null,
          limit: submitted ? null : PREVIEW_LIMIT,
        });
        if (cancelled) return;
        setLookup({ kind: 'done', response, lookingUp: submitted !== '' });
        setMatchError(null);
      } catch (e) {
        if (cancelled) return;
        if (isNetworkError(e)) { setLookup({ kind: 'offline' }); return; }
        // The filter can be a query too, so only an error in the match query is marked in its box
        const inMatch = e instanceof ApiError && e.queryField === 'match_query';
        setMatchError(inMatch && e.span ? { query: submitted, span: e.span } : null);
        setLookup({ kind: 'error', message: errorMessage(e) });
      }
    }, LOOKUP_DELAY_MS);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [filterKey, submitted, refreshKey]);

  return (
    <div style={{ position: 'relative', marginTop: 16, padding: 12, border: '1px solid var(--border)', borderRadius: 6, ...style }}>
      <button onClick={onClose} aria-label="Close Find in queue" style={{ position: 'absolute', top: 8, right: 8, background: 'none', border: 'none', fontSize: 18, cursor: 'pointer', color: 'var(--text-secondary)' }}>×</button>
      <label htmlFor="find-in-queue" style={{ display: 'block', fontSize: 14, marginBottom: 8 }}>Find in queue</label>
      <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
        <SearchInput
          ref={input}
          id="find-in-queue"
          value={match}
          onChange={value => { setMatch(value); if (!value.trim()) setSubmitted(''); }}
          onSubmit={value => setSubmitted(value.trim())}
          placeholder='Only cards matching, e.g. data~"theorem" or c.state=new'
          error={matchError}
          style={{ maxWidth: 'none' }}
          inputStyle={{ fontSize: 14, padding: '8px 12px' }}
        />
        <button onClick={() => setSubmitted(match.trim())} className="touch-target-small">Find</button>
      </div>
      {lookup.kind === 'loading' && <div className="loading-delayed" style={{ fontSize: 13 }}>Looking up the queue…</div>}
      {lookup.kind === 'offline' && <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Needs the server: the cards downloaded for offline review are only some of the queue, and a query can't be searched without it.</div>}
      {lookup.kind === 'error' && <pre style={{ color: 'var(--error)', marginTop: 0, whiteSpace: 'pre-wrap' }}>{lookup.message}</pre>}
      {lookup.kind === 'done' && <QueuePositionList response={lookup.response} onOpen={card => onOpenNote(card.note_id)} lookingUp={lookup.lookingUp} currentCardId={currentCardId} />}
    </div>
  );
}

/** How many cards queue order lists, of those in the queue and of those not shown today. */
const QUEUE_ORDER_LIMIT = 200;

/**
 * The cards matching `query` in the order the default review shows them, for the cards page,
 * followed by those it holds back or would not show today. The order is the queue's, which
 * depends on today's limits and what is due, so it is not a sort of the query language.
 */
export function QueueOrder({ query, onOpen }: { query: string; onOpen: (card: QueueCard) => void }) {
  const [lookup, setLookup] = useState<Lookup>({ kind: 'loading' });
  useEffect(() => {
    let cancelled = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the queue is fetched
    setLookup({ kind: 'loading' });
    getQueuePositions({ filter: null, match_query: query, limit: QUEUE_ORDER_LIMIT })
      .then(response => { if (!cancelled) setLookup({ kind: 'done', response, lookingUp: true }); })
      .catch(e => {
        if (cancelled) return;
        if (isNetworkError(e)) { setLookup({ kind: 'offline' }); return; }
        setLookup({ kind: 'error', message: errorMessage(e) });
      });
    return () => { cancelled = true; };
  }, [query]);
  if (lookup.kind === 'loading' || lookup.kind === 'idle') return <div className="loading-delayed">Loading…</div>;
  if (lookup.kind === 'offline') return <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Queue order needs the server.</div>;
  if (lookup.kind === 'error') return <pre style={{ color: 'var(--error)', marginTop: 0, whiteSpace: 'pre-wrap' }}>{lookup.message}</pre>;
  return <QueuePositionList response={lookup.response} onOpen={onOpen} lookingUp />;
}
