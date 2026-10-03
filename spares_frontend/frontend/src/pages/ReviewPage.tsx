import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import {
  cachedTodayStatistics,
  createReviewSnapshot,
  describeReverted,
  forgetCard,
  getCardsForNote,
  getNote,
  getReviewCardById,
  getReviewConfig,
  getSchedulerRatings,
  getTagByName,
  getTodayStatistics,
  postReview,
  redoEvent,
  searchNotes,
  submitAction,
  tagNote,
  undoEvent,
  updateCards,
} from '../api/client';
import CardRenderer from '../components/CardRenderer';
import { RedoIcon, UndoIcon } from '../components/Icons';
import RecentSearches from '../components/RecentSearches';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { useMediaQuery } from '../hooks/useMediaQuery';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useShowReviewTimer } from '../preferences';
import {
  STATE_LABELS,
  type GetReviewCardResponse,
  type NoteResponse,
  type Rating,
  type ReviewConfig,
  type ReviewFilter,
  type StatisticsResponse,
} from '../types/spares';
import { backPath, formatDuration, sectionLabel } from '../utils';

type Phase = 'landing' | 'loading' | 'front' | 'back' | 'done' | 'summary' | 'error';
type FilterMode = 'query' | 'tagName' | 'tagId';

const FILTER_MODES: { mode: FilterMode; label: string; placeholder: string }[] = [
  { mode: 'query', label: 'Query', placeholder: 'Filter cards due today, e.g. (tag=a limit=5) or tag=b…' },
  { mode: 'tagName', label: 'Tag name', placeholder: 'Filtered tag name' },
  { mode: 'tagId', label: 'Tag id', placeholder: 'Filtered tag id' },
];

// The note editor (CodeMirror) is only needed once a note is opened, so it is loaded separately
const loadNoteDetail = () => import('../components/NoteDetail');
const NoteDetail = lazy(loadNoteDetail);

const actionButton: React.CSSProperties = { padding: '6px 10px', fontSize: 13 };

/** Text, border and background colours for a family of buttons, so each family is recognizable at a glance. Defined in index.css. */
interface Tone { fg: string; border: string; bg: string }

const TONES = {
  red: { fg: 'var(--tone-red-fg)', border: 'var(--tone-red-border)', bg: 'var(--tone-red-bg)' },
  orange: { fg: 'var(--tone-orange-fg)', border: 'var(--tone-orange-border)', bg: 'var(--tone-orange-bg)' },
  green: { fg: 'var(--tone-green-fg)', border: 'var(--tone-green-border)', bg: 'var(--tone-green-bg)' },
  blue: { fg: 'var(--tone-blue-fg)', border: 'var(--tone-blue-border)', bg: 'var(--tone-blue-bg)' },
  purple: { fg: 'var(--tone-purple-fg)', border: 'var(--tone-purple-border)', bg: 'var(--tone-purple-bg)' },
  teal: { fg: 'var(--tone-teal-fg)', border: 'var(--tone-teal-border)', bg: 'var(--tone-teal-bg)' },
  gray: { fg: 'var(--tone-gray-fg)', border: 'var(--tone-gray-border)', bg: 'var(--tone-gray-bg)' },
} satisfies Record<string, Tone>;

function toned(tone: Tone, base: React.CSSProperties = actionButton): React.CSSProperties {
  return { ...base, color: tone.fg, background: tone.bg, border: `1px solid ${tone.border}`, borderRadius: 4, cursor: 'pointer' };
}

const primaryButton: React.CSSProperties = { background: '#175cd3', color: '#fff', border: '1px solid #175cd3', borderRadius: 4, cursor: 'pointer' };

const RATING_TONES: Record<string, Tone> = { again: TONES.red, hard: TONES.orange, good: TONES.green, easy: TONES.blue };

/** Colours a rating by its name, falling back to its position from worst (red) to best (blue). */
function ratingTone(rating: Rating, index: number, count: number): Tone {
  const named = RATING_TONES[rating.description.toLowerCase()];
  if (named) return named;
  const scale = [TONES.red, TONES.orange, TONES.green, TONES.blue];
  return scale[Math.round((index / Math.max(1, count - 1)) * (scale.length - 1))];
}

/** A labelled row of related actions, set apart by a coloured rule on its left. */
function ActionGroup({ label, tone, children }: { label: string; tone: Tone; children: React.ReactNode }) {
  return (
    <div style={{ borderLeft: `3px solid ${tone.border}`, paddingLeft: 10 }}>
      <div style={{ ...sectionLabel, color: tone.fg, marginBottom: 4 }}>{label}</div>
      <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>{children}</div>
    </div>
  );
}

// Hides a rendered side without changing its width, so a Typst side compiled for the hidden
// viewer's width is not recompiled when it is shown.
const hiddenSide: React.CSSProperties = { height: 0, overflow: 'hidden', visibility: 'hidden' };

function msToSeconds(ms: number): number {
  return Math.max(0, Math.floor(ms / 1000));
}

/** When the current card's stopwatches started, and the durations recorded so far (`null` while running). */
interface Stopwatch { recallStart: number; recallMs: number | null; rateStart: number; rateMs: number | null }

/** Counts the recall time on the front, then the rate time on the back, as they would be submitted. */
function ReviewTimer({ stopwatch, flipped }: { stopwatch: Stopwatch; flipped: boolean }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(id);
  }, []);
  const recallMs = stopwatch.recallMs ?? now - stopwatch.recallStart;
  const rateMs = stopwatch.rateMs ?? now - stopwatch.rateStart;
  return (
    <span style={{ fontVariantNumeric: 'tabular-nums' }}>
      Recall {formatDuration(msToSeconds(recallMs))}
      {flipped && <> · Rate {formatDuration(msToSeconds(rateMs))}</>}
    </span>
  );
}

/** Heuristic mirror of `spares_core::search::query_has_limit`. A false positive only means the snapshot endpoint reports the error. */
function queryHasLimit(query: string): boolean {
  return /(^|[\s(])limit\s*=/.test(query);
}

function initialFilter(params: URLSearchParams): { mode: FilterMode; input: string } {
  const tagId = params.get('tagId');
  if (tagId !== null) return { mode: 'tagId', input: tagId };
  const tagName = params.get('tagName');
  if (tagName !== null) return { mode: 'tagName', input: tagName };
  return { mode: 'query', input: params.get('query') ?? '' };
}

/** Inverse of `initialFilter`, so a session's URL can be bookmarked to prefill the same filter. */
function filterParams(mode: FilterMode, input: string): URLSearchParams {
  return input ? new URLSearchParams({ [mode]: input }) : new URLSearchParams();
}

interface RecentFilter { mode: FilterMode; input: string }

const RECENT_FILTER_PREFIX: Record<FilterMode, string> = { query: '', tagName: 'tag: ', tagId: 'tag id: ' };
const sameFilter = (a: RecentFilter, b: RecentFilter) => a.mode === b.mode && a.input === b.input;

export default function ReviewPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();

  const [phase, setPhase] = useState<Phase>('landing');
  const [card, setCard] = useState<GetReviewCardResponse | null>(null);
  const [ratings, setRatings] = useState<Rating[]>([]);
  const [config, setConfig] = useState<ReviewConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  // Shown from the last visit while they are refetched, so the landing page appears straight away
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(
    () => (credentials && cachedTodayStatistics(credentials.schedulerName)) ?? null,
  );
  // The back is rendered hidden once the front is done, so Show Answer only has to reveal it
  const [frontRendered, setFrontRendered] = useState(false);
  const onFrontRendered = useCallback(() => setFrontRendered(true), []);

  // Filter, resolved once per session like the CLI's `resolve_filtered_tag_id`. Prefilled from
  // `?tagId=`, `?tagName=` or `?query=`, e.g. by a tag's Review link, and written back on start.
  const [searchParams, setSearchParams] = useSearchParams();
  const [filterMode, setFilterMode] = useState<FilterMode>(() => initialFilter(searchParams).mode);
  const [filterInput, setFilterInput] = useState(() => initialFilter(searchParams).input);
  const [activeFilter, setActiveFilter] = useState<ReviewFilter | undefined>(undefined);
  const [sessionInfo, setSessionInfo] = useState<string | null>(null);
  const recentFilters = useRecentSearches<RecentFilter>('spares_review_recent_filters', sameFilter);
  const filterInputRef = useRef<HTMLInputElement>(null);

  // Side panels
  const [panelNote, setPanelNote] = useState<NoteResponse | null>(null);
  const [noteLoading, setNoteLoading] = useState(false);
  const notePanelRef = useRef<HTMLDivElement>(null);
  const [keywordResults, setKeywordResults] = useState<{ keyword: string; notes: NoteResponse[] } | null>(null);
  const [dueDatePicker, setDueDatePicker] = useState<'card' | 'note' | null>(null);
  const [dueDateValue, setDueDateValue] = useState('');

  // Session summary
  const [reviewedCount, setReviewedCount] = useState(0);
  const [sessionRecallMs, setSessionRecallMs] = useState(0);
  const [sessionRateMs, setSessionRateMs] = useState(0);
  const [sessionStart, setSessionStart] = useState<number | null>(null);
  const [sessionEnd, setSessionEnd] = useState<number | null>(null);

  // Stopwatches. A duration is `null` until it is recorded, as in the CLI.
  const recallStart = useRef(0);
  const recallDuration = useRef<number | null>(null);
  const rateStart = useRef(0);
  const rateDuration = useRef<number | null>(null);
  // Copied from the refs above whenever they change, for the on-screen timer
  const [stopwatch, setStopwatch] = useState<Stopwatch | null>(null);
  const showTimer = useShowReviewTimer();
  const syncStopwatch = useCallback(() => setStopwatch({
    recallStart: recallStart.current,
    recallMs: recallDuration.current,
    rateStart: rateStart.current,
    rateMs: rateDuration.current,
  }), []);

  // Phones collapse the less common actions so the card and ratings get the screen
  const isNarrow = useMediaQuery('(max-width: 640px)');

  const lastEventId = useRef<number | null>(null);
  const lastActionWasRating = useRef(false);
  // The undo event that Redo reverses, and whether it undid a rating. Any new action clears it.
  const [lastUndo, setLastUndo] = useState<{ eventId: number; wasRating: boolean } | null>(null);

  const tagId = activeFilter && 'FilteredTag' in activeFilter ? activeFilter.FilteredTag.tag_id : null;

  const loadCard = useCallback(async (filter: ReviewFilter | undefined) => {
    setPhase('loading');
    setFrontRendered(false);
    setPanelNote(null);
    setKeywordResults(null);
    setDueDatePicker(null);
    try {
      const next = await postReview(filter);
      if (!next) { setCard(null); setSessionEnd(Date.now()); setPhase('done'); return; }
      setCard(next);
      recallStart.current = Date.now();
      recallDuration.current = null;
      rateDuration.current = null;
      syncStopwatch();
      setPhase('front');
    } catch (e) {
      setError(String(e));
      setPhase('error');
    }
  }, [syncStopwatch]);

  const refreshStatistics = useCallback(() => {
    if (!credentials) return Promise.resolve();
    return getTodayStatistics(credentials.schedulerName).then(setStatistics).catch(console.error);
  }, [credentials]);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    getSchedulerRatings(credentials.schedulerName).then(setRatings).catch(console.error);
    getReviewConfig().then(setConfig).catch(console.error);
    refreshStatistics();
  }, [credentials, navigate, refreshStatistics]);

  // Refresh the day's statistics whenever the session ends
  useEffect(() => {
    if (phase === 'done' || phase === 'summary') refreshStatistics();
  }, [phase, refreshStatistics]);

  async function startReview(mode: FilterMode, rawInput: string) {
    setError(null);
    setSessionInfo(null);
    const input = rawInput.trim();
    let filter: ReviewFilter | undefined;
    try {
      if (mode === 'query' && input) {
        if (queryHasLimit(input)) {
          const snapshot = await createReviewSnapshot(input);
          filter = { FilteredTag: { tag_id: snapshot.tag_id } };
          setSessionInfo(`Reviewing filtered tag \`${snapshot.tag_name}\` (${snapshot.rebuilt ? 'built for today' : 'resumed from earlier today'}, ${snapshot.card_count} cards left)`);
        } else {
          filter = { Query: input };
        }
      } else if (mode === 'tagName' && input) {
        const tag = await getTagByName(input);
        filter = { FilteredTag: { tag_id: tag.id } };
        setSessionInfo(`Reviewing filtered tag \`${tag.name}\``);
      } else if (mode === 'tagId' && input) {
        const id = Number(input);
        if (!Number.isInteger(id)) throw new Error(`Invalid tag id: ${input}`);
        filter = { FilteredTag: { tag_id: id } };
      }
    } catch (e) {
      setError(String(e));
      setPhase('error');
      return;
    }
    if (input) recentFilters.save({ mode, input });
    setSearchParams(filterParams(mode, input), { replace: true });
    setActiveFilter(filter);
    setReviewedCount(0);
    setSessionRecallMs(0);
    setSessionRateMs(0);
    setSessionStart(Date.now());
    setSessionEnd(null);
    lastEventId.current = null;
    lastActionWasRating.current = false;
    setLastUndo(null);
    loadCard(filter);
  }

  /** Leaves the session for the landing page, keeping the current filter prefilled to edit. */
  function switchQuery() {
    if (sessionEnd === null) setSessionEnd(Date.now());
    setStatus(null);
    setPanelNote(null);
    setKeywordResults(null);
    setDueDatePicker(null);
    refreshStatistics();
    setPhase('landing');
  }

  function showAnswer() {
    // Recall may already be recorded if the note was opened before flipping
    if (recallDuration.current === null) {
      const d = Date.now() - recallStart.current;
      recallDuration.current = d;
      setSessionRecallMs(ms => ms + d);
    }
    rateStart.current = Date.now();
    rateDuration.current = null;
    syncStopwatch();
    setPhase('back');
  }

  async function rate(ratingId: number) {
    if (!card || !credentials || recallDuration.current === null) return;
    const rateMs = rateDuration.current ?? Date.now() - rateStart.current;
    const recallSeconds = msToSeconds(recallDuration.current);
    const rateSeconds = msToSeconds(rateMs);
    try {
      const res = await submitAction({
        scheduler_name: credentials.schedulerName,
        action: {
          Rate: {
            card_id: card.card_id,
            rating: ratingId,
            recall_duration: recallSeconds,
            rate_duration: rateSeconds,
            tag_id: tagId,
          },
        },
      });
      setSessionRateMs(ms => ms + rateMs);
      setReviewedCount(c => c + 1);
      lastEventId.current = res.event_id;
      lastActionWasRating.current = true;
      setLastUndo(null);
      setStatus(`Previous card — Recall Duration: ${formatDuration(recallSeconds)} · Rate Duration: ${formatDuration(rateSeconds)}`);
      loadCard(activeFilter);
    } catch (e) {
      setError(String(e));
      setPhase('error');
    }
  }

  /** Runs an action that removes the current card from today's queue, then advances. */
  async function runCardAction(action: () => Promise<number | null>, message?: string) {
    try {
      lastEventId.current = await action();
      lastActionWasRating.current = false;
      setLastUndo(null);
      setStatus(message ?? null);
      loadCard(activeFilter);
    } catch (e) {
      setStatus(String(e));
    }
  }

  function buryCard() {
    if (!card || !credentials) return;
    const { schedulerName } = credentials;
    runCardAction(
      async () => (await submitAction({ scheduler_name: schedulerName, action: { Bury: { card_id: card.card_id } } })).event_id,
      'Card buried.',
    );
  }

  function buryNote() {
    if (!card) return;
    runCardAction(async () => {
      const cards = await getCardsForNote(card.note_id);
      const ids = cards.filter(c => c.special_state === null).map(c => c.id);
      return (await updateCards({ selector: { Ids: ids }, special_state: 'Buried' })).event_id;
    }, 'Note buried.');
  }

  function buryUntilLaterToday() {
    if (!card) return;
    // `due` is used as the burial timestamp so buried cards come back in FIFO order
    runCardAction(
      async () => (await updateCards({
        selector: { Ids: [card.card_id] },
        special_state: 'BuriedUntilLaterToday',
        due: new Date().toISOString(),
      })).event_id,
      'Card due date set to end of today.',
    );
  }

  function suspendCard() {
    if (!card) return;
    runCardAction(
      async () => (await updateCards({ selector: { Ids: [card.card_id] }, special_state: 'Suspended' })).event_id,
      'Card suspended.',
    );
  }

  function suspendNote() {
    if (!card) return;
    runCardAction(async () => {
      const cards = await getCardsForNote(card.note_id);
      return (await updateCards({ selector: { Ids: cards.map(c => c.id) }, special_state: 'Suspended' })).event_id;
    }, 'Note suspended.');
  }

  function forget() {
    if (!card) return;
    runCardAction(async () => (await forgetCard(card.card_id)).event_id, 'Card forgotten (scheduling reset).');
  }

  function setDueDate(target: 'card' | 'note', due: Date) {
    if (!card) return;
    runCardAction(async () => {
      let ids = [card.card_id];
      if (target === 'note') {
        // Only push sibling cards later, never pull them earlier
        const cards = await getCardsForNote(card.note_id);
        ids = cards.filter(c => new Date(c.due) <= due).map(c => c.id);
      }
      return (await updateCards({ selector: { Ids: ids }, due: due.toISOString() })).event_id;
    }, 'Due date updated.');
  }

  function setDueDateFromPicker() {
    if (!dueDatePicker || !dueDateValue) return;
    // Midnight UTC of the picked date, matching the CLI's date prompt
    setDueDate(dueDatePicker, new Date(`${dueDateValue}T00:00:00Z`));
  }

  function setDueDateIn(target: 'card' | 'note') {
    if (!config) return;
    setDueDate(target, new Date(Date.now() + config.set_card_due_date_duration * 1000));
  }

  async function flagNote() {
    if (!card || !config) return;
    try {
      lastEventId.current = await tagNote(card.note_id, config.flagged_tag_name);
      lastActionWasRating.current = false;
      setLastUndo(null);
      setStatus(`Note tagged \`${config.flagged_tag_name}\`.`);
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function undo() {
    if (phase === 'back') {
      // Undo the flip: discard the recorded recall duration and restart the recall timer
      if (recallDuration.current !== null) {
        const d = recallDuration.current;
        setSessionRecallMs(ms => Math.max(0, ms - d));
      }
      recallDuration.current = null;
      rateDuration.current = null;
      recallStart.current = Date.now();
      syncStopwatch();
      setPhase('front');
      return;
    }
    try {
      // Use the tracked event id so syncs from elsewhere don't cause the wrong event to be undone
      const res = await undoEvent(lastEventId.current);
      lastEventId.current = null;
      setStatus(res ? describeReverted('Undone', res.undone_events) : 'No event to undo.');
      const undoId = res?.undo_event_ids[0];
      setLastUndo(undoId === undefined ? null : { eventId: undoId, wasRating: lastActionWasRating.current });
      if (lastActionWasRating.current) setReviewedCount(c => Math.max(0, c - 1));
      lastActionWasRating.current = false;
      // The next card will be the one that was just undone
      loadCard(activeFilter);
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function redo() {
    if (!lastUndo) {
      setStatus('Nothing to redo.');
      return;
    }
    try {
      const res = await redoEvent(lastUndo.eventId);
      setLastUndo(null);
      if (!res) {
        setStatus('No event to redo.');
        return;
      }
      setStatus(describeReverted('Redone', res.redone_events));
      // So that Undo reverses exactly this redo
      lastEventId.current = res.redo_event_ids[0] ?? null;
      lastActionWasRating.current = lastUndo.wasRating;
      if (lastUndo.wasRating) setReviewedCount(c => c + 1);
      // The redone action moved the card on again
      loadCard(activeFilter);
    } catch (e) {
      setStatus(String(e));
    }
  }

  async function openNote(noteId: number) {
    // Seeing the note reveals the answer, so recall ends here. If the card is already flipped,
    // freeze the rate timer since editing the note isn't representative of a normal review.
    if (card && noteId === card.note_id) {
      if (phase === 'back') {
        if (rateDuration.current === null) rateDuration.current = Date.now() - rateStart.current;
      } else if (recallDuration.current === null) {
        const d = Date.now() - recallStart.current;
        recallDuration.current = d;
        setSessionRecallMs(ms => ms + d);
      }
      syncStopwatch();
    }
    setNoteLoading(true);
    try {
      setPanelNote(await getNote(noteId));
    } catch (e) {
      setStatus(String(e));
    } finally {
      setNoteLoading(false);
    }
  }

  function toggleCardNote() {
    if (!card) return;
    if (panelNote?.id === card.note_id) setPanelNote(null);
    else openNote(card.note_id);
  }

  // The editor opens below a card that can be taller than the viewport, so bring it into view
  const panelNoteId = panelNote?.id;
  useEffect(() => {
    if (panelNoteId !== undefined) notePanelRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, [panelNoteId]);

  // Load the Typst compiler while the filter is picked, so the first Typst card isn't delayed by it
  const onLanding = phase === 'landing';
  useEffect(() => {
    if (onLanding) import('../typst/compiler').then(({ warmUpTypst }) => warmUpTypst()).catch(console.error);
  }, [onLanding]);

  // Load the note editor while a card is shown, so opening the note isn't delayed by it
  const reviewing = phase === 'front' || phase === 'back';
  useEffect(() => {
    if (reviewing) loadNoteDetail().catch(console.error);
  }, [reviewing]);

  // After leaving a session, focus the prefilled filter so a new one can be typed straight away
  const returnedToLanding = phase === 'landing' && sessionStart !== null;
  useEffect(() => {
    if (returnedToLanding) filterInputRef.current?.select();
  }, [returnedToLanding]);

  /** Mirrors the CLI's Sync Note: re-render the note, then refresh the current card in place. */
  async function onNoteUpdated(updated: NoteResponse) {
    setPanelNote(updated);
    if (!card || updated.id !== card.note_id) return;
    try {
      // NoteDetail has already regenerated the note's rendered files
      const refreshed = await getReviewCardById(card.card_id, activeFilter);
      if (refreshed) {
        setCard(refreshed);
        setStatus('Current card refreshed after sync (card order may have changed).');
      } else {
        setStatus('Current card was deleted during sync. Advancing to next card.');
        loadCard(activeFilter);
      }
    } catch (e) {
      setStatus(`Failed to refresh card after sync: ${String(e)}`);
    }
  }

  /** Tapping the front shows the answer, for touchscreens without the Space shortcut. */
  function onFrontClick(e: React.MouseEvent) {
    if (phase !== 'front' || card?.cli) return;
    if (e.target instanceof Element && e.target.closest('a, button, input, select, textarea, summary, .cm-editor')) return;
    // Selecting text shouldn't flip the card
    if (window.getSelection()?.toString()) return;
    showAnswer();
  }

  async function browseKeyword(keyword: string) {
    try {
      const notes = await searchNotes(`linked_to_keyword="${keyword}"`);
      setKeywordResults({ keyword, notes });
    } catch (e) {
      setStatus(String(e));
    }
  }

  // Keyboard shortcuts — re-bind on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) return;
      if (target instanceof Element && target.closest('.cm-editor')) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (phase !== 'front' && phase !== 'back') return;
      // Space shows the answer, so it must not also click a button (or toggle a section) left
      // focused by a mouse click, e.g. Bury Card when pressed on the back
      if (e.code === 'Space' && target instanceof Element && target.closest('button, summary')) e.preventDefault();
      if (phase === 'front' && e.code === 'Space') {
        e.preventDefault();
        showAnswer();
        return;
      }
      if (phase === 'back') {
        const idx = parseInt(e.key, 10) - 1;
        if (idx >= 0 && idx < ratings.length) { rate(ratings[idx].id); return; }
      }
      switch (e.key) {
        case 'u': undo(); break;
        case 'U': redo(); break;
        case 'b': buryCard(); break;
        case 's': suspendCard(); break;
        case 'e': toggleCardNote(); break;
        case 'q': switchQuery(); break;
      }
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  });

  const cardCounts = card
    ? Object.entries(card.cards_left_by_state)
        .filter(([, count]) => count > 0)
        .map(([stateId, count]) => `${STATE_LABELS[Number(stateId)] ?? stateId}: ${count}`)
        .join(' · ')
    : '';

  const dueInLabel = config ? formatDuration(config.set_card_due_date_duration) : '…';
  const sessionStarted = sessionStart !== null;

  const summary = sessionStarted && (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: 16, marginTop: 24, textAlign: 'left', display: 'inline-block' }}>
      <div style={sectionLabel}>Session</div>
      <div>Cards reviewed: {reviewedCount}</div>
      <div>Total time: {formatDuration(msToSeconds((sessionEnd ?? sessionStart ?? 0) - (sessionStart ?? 0)))}</div>
      <div>Recall time: {formatDuration(msToSeconds(sessionRecallMs))}</div>
      <div>Rate time: {formatDuration(msToSeconds(sessionRateMs))}</div>
      {statistics && (
        <>
          <div style={{ ...sectionLabel, marginTop: 12 }}>Today</div>
          <div>Cards studied: {statistics.cards_studied_count}</div>
          <div>Recall time: {formatDuration(statistics.recall_duration)}</div>
          <div>Rate time: {formatDuration(statistics.rate_duration)}</div>
        </>
      )}
    </div>
  );

  return (
    <div className="page">
      <Navbar onLogout={logout} />
      <h2 style={{ marginBottom: 16 }}>Review</h2>

      {phase === 'landing' && (
        <div style={{ marginTop: 48 }}>
          {sessionStarted && <div style={{ marginBottom: 24 }}>{summary}</div>}
          {statistics && (
            <div style={{ display: 'flex', gap: 16, marginBottom: 32, flexWrap: 'wrap' }}>
              {Object.entries(statistics.due_count_by_state)
                .filter(([, count]) => count > 0)
                .map(([stateId, count]) => (
                  <div key={stateId} style={{ border: '1px solid var(--border)', borderRadius: 6, padding: '12px 20px', textAlign: 'center' }}>
                    <div style={{ fontSize: 24, fontWeight: 600 }}>{count}</div>
                    <div style={{ fontSize: 13, color: 'var(--text-secondary)' }}>{STATE_LABELS[Number(stateId)] ?? stateId}</div>
                  </div>
                ))}
              {Object.values(statistics.due_count_by_state).every(c => c === 0) && (
                <p style={{ color: 'var(--text-secondary)' }}>Nothing due to review.</p>
              )}
            </div>
          )}
          <div style={{ display: 'flex', gap: 16, marginBottom: 8, fontSize: 14 }}>
            {FILTER_MODES.map(({ mode, label }) => (
              <label key={mode} style={{ cursor: 'pointer' }}>
                <input
                  type="radio"
                  name="filter-mode"
                  checked={filterMode === mode}
                  onChange={() => setFilterMode(mode)}
                  style={{ marginRight: 4 }}
                />
                {label}
              </label>
            ))}
          </div>
          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
            <input
              ref={filterInputRef}
              type={filterMode === 'tagId' ? 'number' : 'text'}
              placeholder={FILTER_MODES.find(m => m.mode === filterMode)?.placeholder}
              value={filterInput}
              onChange={e => setFilterInput(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') startReview(filterMode, filterInput); }}
              style={{ flex: 1, padding: '8px 12px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 }}
            />
            <button onClick={() => startReview(filterMode, filterInput)} className="touch-target" style={{ ...primaryButton, padding: '8px 24px', fontSize: 14 }}>
              Start Review
            </button>
          </div>
          <RecentSearches
            recent={recentFilters.recent}
            label={f => `${RECENT_FILTER_PREFIX[f.mode]}${f.input}`}
            onSelect={f => { setFilterMode(f.mode); setFilterInput(f.input); startReview(f.mode, f.input); }}
            onRemove={recentFilters.remove}
            selectTitle="Start reviewing this filter"
          />
          {filterMode === 'query' && (
            <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: 0 }}>
              Queries using <code>limit</code> are saved to a filtered tag once per day, so running the same query again later that day resumes it.
            </p>
          )}
        </div>
      )}

      {phase === 'loading' && <div>Loading…</div>}

      {(phase === 'done' || phase === 'summary') && (
        <div style={{ textAlign: 'center', marginTop: 80, color: 'var(--text-secondary)' }}>
          {phase === 'done' && <p>Nothing left to review.</p>}
          {status && <p style={{ fontSize: 13 }}>{status}</p>}
          {summary}
          <p style={{ marginTop: 24, display: 'flex', gap: 16, justifyContent: 'center' }}>
            <button onClick={() => { setStatus(null); setPhase('landing'); }}>New session</button>
            <Link to="/notes">View notes</Link>
          </p>
        </div>
      )}

      {phase === 'error' && (
        <div style={{ color: 'var(--error)' }}>
          Error: {error}
          <button onClick={() => (card || sessionStarted ? loadCard(activeFilter) : setPhase('landing'))} style={{ marginLeft: 12 }}>Retry</button>
          <button onClick={() => setPhase('landing')} style={{ marginLeft: 8 }}>Back</button>
        </div>
      )}

      {(phase === 'front' || phase === 'back') && card && (
        <>
          {sessionInfo && <div style={{ marginBottom: 8, fontSize: 13, color: 'var(--text-secondary)' }}>{sessionInfo}</div>}
          <div style={{ marginBottom: 8, fontSize: 13, color: 'var(--text-muted)', display: 'flex', gap: 8, flexWrap: 'wrap' }}>
            <span>Note {card.note_id} · Card {card.card_id} (#{card.card_order}) · {card.parser_name}</span>
            {cardCounts && <span style={{ color: 'var(--text-secondary)' }}>{cardCounts}</span>}
            <span className="review-meta-right" style={{ marginLeft: 'auto' }}>
              Reviewed {reviewedCount} · ~{formatDuration(card.time_estimate)} left
            </span>
            {showTimer && stopwatch && <ReviewTimer stopwatch={stopwatch} flipped={phase === 'back'} />}
          </div>

          {/* Like the CLI, the front is closed when the card is flipped. It stays mounted so undoing
              the flip shows it again without rerendering. */}
          <div style={phase === 'back' ? hiddenSide : undefined} aria-hidden={phase === 'back'}>
            {card.cli ? (
              <div style={{ border: '1px solid var(--border)', borderRadius: 4, padding: 16, marginBottom: 16 }}>
                <pre style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{card.cli.surrounding}</pre>
                <p style={{ color: 'var(--warning)', fontSize: 13, marginBottom: 0 }}>
                  This card is reviewed by running <code>{card.cli.exec}</code>, which only the CLI can do. Bury it or review it with <code>spares card review</code>.
                </p>
              </div>
            ) : (
              <div onClick={onFrontClick} style={{ border: '1px solid var(--border)', borderRadius: 4, overflow: 'hidden', marginBottom: 16, cursor: phase === 'front' ? 'pointer' : undefined }}>
                <CardRenderer path={card.card_front_rendered_path} parserName={card.parser_name} source={card.browser_sources?.card_front} onReady={onFrontRendered} />
              </div>
            )}
          </div>

          {(phase === 'back' || (frontRendered && !card.cli)) && (
            <div style={phase === 'back' ? undefined : hiddenSide} aria-hidden={phase !== 'back'}>
              <div style={{ border: '1px solid var(--border)', borderRadius: 4, overflow: 'hidden', marginBottom: 16, background: 'var(--surface)' }}>
                <CardRenderer path={backPath(card)} parserName={card.parser_name} source={card.browser_sources?.card_back} />
              </div>
            </div>
          )}

          {((phase === 'front' && !card.cli) || phase === 'back') && (
            <div className="review-answer-bar">
              {phase === 'front' ? (
                <button onClick={showAnswer} className="touch-target" style={{ ...primaryButton, width: '100%', padding: 12, fontSize: 15, fontWeight: 600 }}>
                  Show Answer <span className="key-hint" style={{ fontSize: 12, fontWeight: 400 }}>(Space)</span>
                </button>
              ) : (
                <div style={{ display: 'flex', gap: 8 }}>
                  {ratings.map((r, i) => (
                    <button key={r.id} onClick={() => rate(r.id)} className="touch-target" style={{ ...toned(ratingTone(r, i, ratings.length), { flex: 1, padding: 12, fontSize: 15, fontWeight: 600 }) }}>
                      {r.description}
                      <span className="key-hint" style={{ display: 'block', fontSize: 12, fontWeight: 400 }}>({i + 1})</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}

          {status && <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-secondary)' }}>{status}</div>}

          {noteLoading && <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>Loading note…</div>}
          {panelNote && (
            <div ref={notePanelRef} style={{ marginTop: 16, scrollMarginTop: 16 }}>
              <Suspense fallback={<div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading note…</div>}>
                <NoteDetail
                  key={panelNote.id}
                  note={panelNote}
                  onClose={() => setPanelNote(null)}
                  onNoteUpdated={onNoteUpdated}
                />
              </Suspense>
            </div>
          )}

          <div style={{ marginTop: 20, display: 'flex', flexDirection: 'column', gap: 14 }}>
            {isNarrow && (
              <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                <button onClick={undo} className="touch-target" style={toned(TONES.gray)}><UndoIcon />Undo</button>
                {lastUndo && <button onClick={redo} className="touch-target" style={toned(TONES.gray)}><RedoIcon />Redo</button>}
                <button onClick={toggleCardNote} className="touch-target" style={toned(TONES.blue)}>{panelNote?.id === card.note_id ? 'Close Note' : 'Edit Note'}</button>
                <button onClick={buryCard} className="touch-target" style={toned(TONES.orange)}>Bury Card</button>
                <button onClick={suspendCard} className="touch-target" style={toned(TONES.purple)}>Suspend Card</button>
              </div>
            )}
            {/* Remount when the width class changes so it opens by default on wide screens */}
            <details key={String(isNarrow)} open={!isNarrow} className="review-more-actions">
              <summary>More actions</summary>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: isNarrow ? 8 : 0 }}>
                <div style={{ display: 'flex', gap: 14, flexWrap: 'wrap' }}>
                  <div style={{ flex: 1 }}>
                    <ActionGroup label="Note" tone={TONES.blue}>
                      {!isNarrow && <button onClick={undo} style={toned(TONES.gray)}><UndoIcon />Undo <span className="key-hint">(u)</span></button>}
                      {!isNarrow && lastUndo && <button onClick={redo} style={toned(TONES.gray)}><RedoIcon />Redo <span className="key-hint">(U)</span></button>}
                      {!isNarrow && <button onClick={toggleCardNote} style={toned(TONES.blue)}>{panelNote?.id === card.note_id ? 'Close Note' : 'Edit Note'} <span className="key-hint">(e)</span></button>}
                      <button onClick={flagNote} disabled={!config} style={toned(TONES.blue)}>
                        Tag to modify later{config ? ` (${config.flagged_tag_name})` : ''}
                      </button>
                    </ActionGroup>
                  </div>
                  <ActionGroup label="Session" tone={TONES.gray}>
                    <button onClick={switchQuery} style={toned(TONES.gray)}>Switch query <span className="key-hint">(q)</span></button>
                    <button onClick={() => { setSessionEnd(Date.now()); setPhase('summary'); }} style={toned(TONES.gray)}>End session</button>
                  </ActionGroup>
                </div>
                <ActionGroup label="Remove from today's queue" tone={TONES.orange}>
                  {!isNarrow && <button onClick={buryCard} style={toned(TONES.orange)}>Bury Card <span className="key-hint">(b)</span></button>}
                  <button onClick={buryNote} style={toned(TONES.orange)}>Bury Note (card + siblings)</button>
                  <button onClick={buryUntilLaterToday} style={toned(TONES.orange)}>Bury Until Later Today</button>
                  {!isNarrow && <button onClick={suspendCard} style={toned(TONES.purple)}>Suspend Card <span className="key-hint">(s)</span></button>}
                  <button onClick={suspendNote} style={toned(TONES.purple)}>Suspend Note (card + siblings)</button>
                  <button onClick={forget} style={toned(TONES.red)}>Forget Card</button>
                </ActionGroup>
                <ActionGroup label="Reschedule" tone={TONES.teal}>
                  <button onClick={() => setDueDatePicker('card')} style={toned(TONES.teal)}>Set Card Due Date…</button>
                  <button onClick={() => setDueDateIn('card')} disabled={!config} style={toned(TONES.teal)}>Set Card Due Date in {dueInLabel}</button>
                  <button onClick={() => setDueDatePicker('note')} style={toned(TONES.teal)}>Set Note Due Date…</button>
                  <button onClick={() => setDueDateIn('note')} disabled={!config} style={toned(TONES.teal)}>Set Note Due Date in {dueInLabel}</button>
                  {dueDatePicker && (
                    <div style={{ display: 'flex', gap: 8, alignItems: 'center', fontSize: 13, flexBasis: '100%' }}>
                      <span>{dueDatePicker === 'card' ? 'Card' : 'Note'} due date:</span>
                      <input type="date" value={dueDateValue} onChange={e => setDueDateValue(e.target.value)} />
                      <button onClick={setDueDateFromPicker} disabled={!dueDateValue} style={toned(TONES.teal)}>Set</button>
                      <button onClick={() => setDueDatePicker(null)} style={toned(TONES.gray)}>Cancel</button>
                    </div>
                  )}
                </ActionGroup>
              </div>
            </details>

            {card.keywords.length > 0 && (
              <details>
                <summary style={{ cursor: 'pointer', fontSize: 13 }}>Browse Keywords ({card.keywords.length})</summary>
                <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 8 }}>
                  {card.keywords.map(k => (
                    <button key={k} onClick={() => browseKeyword(k)} style={toned(TONES.gray)}>{k}</button>
                  ))}
                </div>
                {keywordResults && (
                  <div style={{ marginTop: 8, fontSize: 13 }}>
                    <div style={sectionLabel}>Notes linked to “{keywordResults.keyword}”</div>
                    {keywordResults.notes.length === 0 && <div style={{ color: 'var(--text-muted)' }}>No notes found.</div>}
                    {keywordResults.notes.map(n => (
                      <div key={n.id}>
                        <a href="#" onClick={e => { e.preventDefault(); openNote(n.id); }}>Note {n.id}</a>
                        <span style={{ color: 'var(--text-muted)' }}> — {n.data.slice(0, 80)}</span>
                      </div>
                    ))}
                  </div>
                )}
              </details>
            )}

            {card.linked_notes.length > 0 && (
              <details>
                <summary style={{ cursor: 'pointer', fontSize: 13 }}>Open Linked Notes ({card.linked_notes.length})</summary>
                <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 8 }}>
                  {card.linked_notes.map(ln => (
                    <button key={`${ln.searched_keyword}-${ln.note_id}`} onClick={() => openNote(ln.note_id)} style={toned(TONES.gray)}>
                      {ln.searched_keyword} ({ln.note_id})
                    </button>
                  ))}
                </div>
              </details>
            )}
          </div>

        </>
      )}
    </div>
  );
}
