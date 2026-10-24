import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import {
  ApiError,
  cachedReviewHistory,
  cachedTodayStatistics,
  createReviewSnapshot,
  describeReverted,
  forgetCard,
  getCardsForNote,
  getNote,
  getReviewCardById,
  getReviewConfig,
  getReviewHistory,
  getSchedulerRatings,
  getTagByName,
  getTodayStatistics,
  isNetworkError,
  nextReviewAvailableAt,
  postReview,
  redoEvent,
  searchNotes,
  tagNote,
  undoEvent,
  updateCards,
} from '../api/client';
import { queueKey } from '../offline/db';
import {
  cancelPendingAction,
  forgetQueuedCard,
  getOfflineQueue,
  nextOfflineCard,
  nextOfflineCardAvailableAt,
  performStudyAction,
  precompileNextCard,
  refreshQueue,
  returnToQueue,
  syncPendingActions,
  type ActionResult,
} from '../offline/review';
import CardInfo from '../components/CardInfo';
import CardRenderer from '../components/CardRenderer';
import CodeText from '../components/CodeText';
import { RedoIcon, UndoIcon } from '../components/Icons';
import FindInQueue from '../components/QueuePositions';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import { TypedAnswerInputs, TypedAnswerResult } from '../components/TypedAnswer';
import AudioBar from '../components/AudioBar';
import ReviewHeatmap from '../components/ReviewHeatmap';
import Navbar from '../components/Navbar';
import { useAudioPlayer } from '../hooks/useAudioPlayer';
import { useAuth } from '../hooks/useAuth';
import { dayAfter, formatBuryUntil, localDateString, useBuryUntilDate } from '../hooks/useBuryUntilDate';
import { useMediaQuery } from '../hooks/useMediaQuery';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { TYPED_ANSWER_SHORTCUTS, type Keybinding } from '../help/shortcuts';
import {
  CARD_ZOOMS, PAGE_WIDTH_LABELS, PAGE_WIDTHS, PAGES_PER_ROW, PAGES_PER_ROW_LABELS, setCardZoom, setPageWidth, setPagesPerRow,
  useCardZoom, usePageWidth, usePagesPerRow, useShowReviewTimer, useVimKeybindings,
} from '../preferences';
import { rememberReviewWidth } from '../typst/width';
import {
  STATE_LABELS,
  type GetReviewCardResponse,
  type NoteResponse,
  type Rating,
  type ReviewConfig,
  type ReviewFilter,
  type ReviewHistoryResponse,
  type StatisticsResponse,
} from '../types/spares';
import { backPath, backWebPath, errorMessage, formatDuration, formatNoteId, keepShown, noteChangedSince, sectionLabel } from '../utils';

/** A wait of `ms` milliseconds, like "4m 05s". */
function formatWait(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(seconds / 60);
  return minutes > 0 ? `${minutes}m ${String(seconds % 60).padStart(2, '0')}s` : `${seconds}s`;
}

type Phase = 'landing' | 'loading' | 'front' | 'back' | 'waiting' | 'done' | 'error';
type FilterMode = 'query' | 'tagName' | 'tagId';
const filterInputStyle: React.CSSProperties = { padding: '8px 12px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };

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

/** How long typing in the filter pauses before the next card for it is compiled. */
const PRECOMPILE_DELAY_MS = 500;

/** Border, padding and line heights of a due count card on the landing page. */
const DUE_CARD_HEIGHT = 2 + 12 + 26 + 16;

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

/** What a session reviews, shown above its cards when nothing more specific was resolved for it. */
function describeFilter(mode: FilterMode, input: string): string {
  if (!input) return 'Reviewing all due cards';
  if (mode === 'query') return `Reviewing \`${input}\``;
  if (mode === 'tagName') return `Reviewing filtered tag \`${input}\``;
  return `Reviewing filtered tag with id ${input}`;
}

const RECENT_FILTER_PREFIX: Record<FilterMode, string> = { query: '', tagName: 'tag: ', tagId: 'tag id: ' };
const sameFilter = (a: RecentFilter, b: RecentFilter) => a.mode === b.mode && a.input === b.input;

export default function ReviewPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();

  const [phase, setPhase] = useState<Phase>('landing');
  const [card, setCard] = useState<GetReviewCardResponse | null>(null);
  // Bumped when the card is refreshed in place, since its rendered files keep their paths when the
  // note is rendered again
  const [cardVersion, setCardVersion] = useState(0);
  // What was typed into each blank of the card's typed answer
  const [typedAnswers, setTypedAnswers] = useState<string[]>([]);
  const audioPlayer = useAudioPlayer();
  const [ratings, setRatings] = useState<Rating[]>([]);
  const [config, setConfig] = useState<ReviewConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A query that can't be searched, marked in the box it was typed in
  const [queryError, setQueryError] = useState<SearchInputError | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  // The status shown in full on phones, where it is otherwise cut to one line. A new status starts cut.
  const [expandedStatus, setExpandedStatus] = useState<string | null>(null);
  const statusExpanded = status !== null && expandedStatus === status;
  // Shown from the last visit while they are refetched, so the landing page appears straight away
  const [statistics, setStatistics] = useState<StatisticsResponse | null>(
    () => (credentials && cachedTodayStatistics(credentials.schedulerName)) ?? null,
  );
  const [history, setHistory] = useState<ReviewHistoryResponse | null>(() => cachedReviewHistory() ?? null);
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
  // Find in queue, for the filter typed. Open when its query is in the URL, e.g. from a bookmark.
  const [showQueue, setShowQueue] = useState(() => searchParams.has('match'));

  // Side panels
  const [panelNote, setPanelNote] = useState<NoteResponse | null>(null);
  const [noteLoading, setNoteLoading] = useState(false);
  const notePanelRef = useRef<HTMLDivElement>(null);
  const answerBarRef = useRef<HTMLDivElement>(null);
  const [keywordResults, setKeywordResults] = useState<{ keyword: string; notes: NoteResponse[] } | null>(null);
  const [dueDatePicker, setDueDatePicker] = useState<'card' | 'note' | null>(null);
  const [dueDateValue, setDueDateValue] = useState('');
  // Asking for the date to bury cards until: to bury the card once it is picked, or only to change it
  const [buryUntilPicker, setBuryUntilPicker] = useState<'bury' | 'change' | null>(null);
  const [buryUntilValue, setBuryUntilValue] = useState('');

  // Session summary
  const [reviewedCount, setReviewedCount] = useState(0);
  const [sessionRecallMs, setSessionRecallMs] = useState(0);
  const [sessionRateMs, setSessionRateMs] = useState(0);
  const [sessionStart, setSessionStart] = useState<number | null>(null);
  const [sessionEnd, setSessionEnd] = useState<number | null>(null);
  // Whether the session has shown a card, so one that failed to start, e.g. on a query error, has no summary
  const [sessionShowedCard, setSessionShowedCard] = useState(false);
  // When the next learning card can be shown, while waiting for it with nothing else to review
  const [waitingUntil, setWaitingUntil] = useState<number | null>(null);
  const [, setWaitTick] = useState(0);

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
  // Laptops and wider have room for the note editor beside the card rather than below it
  const isWide = useMediaQuery('(min-width: 1100px)');

  const lastEventId = useRef<number | null>(null);
  const lastActionWasRating = useRef(false);
  // The filter as entered, which identifies the queue downloaded for it (see src/offline)
  const sessionKey = useRef(queueKey('query', ''));
  // The same key, as state so the date to bury cards until is read again for each session
  const [sessionKeyShown, setSessionKeyShown] = useState(sessionKey.current);
  const [buryUntil, setBuryUntil] = useBuryUntilDate(sessionKeyShown);
  // Whether the current card came from the downloaded queue because the server could not be reached
  const [offline, setOffline] = useState(false);
  // The last rating or bury if it is yet to be sent, so Undo can take it back before it is
  const lastPendingAction = useRef<{ clientId: string; card: GetReviewCardResponse } | null>(null);
  // The undo event that Redo reverses, and whether it undid a rating. Any new action clears it.
  const [lastUndo, setLastUndo] = useState<{ eventId: number; wasRating: boolean; ratedCardId: number | null } | null>(null);
  // The card rated last, for Previous Card Info. `version` is bumped on every rating so the panel
  // refetches even when the same card comes back, e.g. during relearning.
  const [previousCard, setPreviousCard] = useState<{ id: number; version: number } | null>(null);
  const [showPreviousCardInfo, setShowPreviousCardInfo] = useState(false);
  // Find in queue, during a session, for its filter
  const [showFindInQueue, setShowFindInQueue] = useState(false);

  const tagId = activeFilter && 'FilteredTag' in activeFilter ? activeFilter.FilteredTag.tag_id : null;

  const loadCard = useCallback(async (filter: ReviewFilter | undefined) => {
    setPhase('loading');
    setFrontRendered(false);
    setTypedAnswers([]);
    setPanelNote(null);
    setKeywordResults(null);
    setDueDatePicker(null);
    setBuryUntilPicker(null);
    try {
      let next: GetReviewCardResponse | null;
      try {
        // Actions taken offline go first, so the server does not show those cards again
        const synced = await syncPendingActions();
        const pending = lastPendingAction.current;
        if (pending && synced.applied.has(pending.clientId)) {
          // Undo now undoes it on the server
          lastEventId.current = synced.applied.get(pending.clientId) ?? null;
          lastPendingAction.current = null;
        }
        next = await postReview(filter);
        setOffline(false);
        if (!next) {
          const availableAt = await nextReviewAvailableAt(filter);
          if (availableAt) {
            setCard(null);
            setWaitingUntil(Date.parse(availableAt));
            setPhase('waiting');
            return;
          }
        }
      } catch (e) {
        if (!isNetworkError(e)) throw e;
        const key = sessionKey.current;
        if (!(await getOfflineQueue(key))) {
          setError(`${errorMessage(e)}. No cards were downloaded for this filter to review offline.`);
          setPhase('error');
          return;
        }
        next = await nextOfflineCard(key);
        setOffline(true);
        if (!next) {
          // Waits like an online review, trying the server again once the time comes
          const availableAt = await nextOfflineCardAvailableAt(key);
          if (availableAt) {
            setCard(null);
            setWaitingUntil(availableAt.getTime());
            setPhase('waiting');
            return;
          }
        }
      }
      if (!next) { setCard(null); setSessionEnd(Date.now()); setPhase('done'); return; }
      setCard(next);
      setSessionShowedCard(true);
      recallStart.current = Date.now();
      recallDuration.current = null;
      rateDuration.current = null;
      syncStopwatch();
      setPhase('front');
    } catch (e) {
      setError(errorMessage(e));
      if (filter && 'Query' in filter && e instanceof ApiError && e.span) {
        setQueryError({ query: filter.Query, span: e.span });
        setPhase('landing');
        return;
      }
      setPhase('error');
    }
  }, [syncStopwatch]);

  // Autoplays the clips each side shows, as the card is shown and flipped. Replay plays both.
  const { play: playAudio, stop: stopAudio } = audioPlayer;
  useEffect(() => {
    if (!card || (phase !== 'front' && phase !== 'back')) { stopAudio(); return; }
    const clips = phase === 'front' ? card.audio?.front : card.audio?.back;
    if (clips?.length) playAudio(clips);
    else stopAudio();
  }, [card, phase, playAudio, stopAudio]);
  const replayAudio = useCallback(() => {
    if (!card?.audio) return;
    playAudio(phase === 'back' ? [...card.audio.front, ...card.audio.back] : card.audio.front);
  }, [card, phase, playAudio]);
  const playAudioClip = useCallback((path: string) => playAudio([path]), [playAudio]);

  const refreshStatistics = useCallback(() => {
    if (!credentials) return Promise.resolve();
    getReviewHistory().then(setHistory).catch(console.error);
    return getTodayStatistics(credentials.schedulerName).then(setStatistics).catch(console.error);
  }, [credentials]);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    getSchedulerRatings(credentials.schedulerName).then(setRatings).catch(console.error);
    getReviewConfig().then(setConfig).catch(console.error);
    refreshStatistics();
  }, [credentials, navigate, refreshStatistics]);

  // While waiting for a learning card, count down to it and then show it
  useEffect(() => {
    if (phase !== 'waiting' || waitingUntil === null) return;
    const tick = setInterval(() => setWaitTick(t => t + 1), 1000);
    const show = setTimeout(() => loadCard(activeFilter), Math.max(0, waitingUntil - Date.now()));
    return () => { clearInterval(tick); clearTimeout(show); };
  }, [phase, waitingUntil, loadCard, activeFilter]);

  // Refresh the day's statistics whenever the session ends
  useEffect(() => {
    if (phase === 'done') refreshStatistics();
  }, [phase, refreshStatistics]);

  async function startReview(mode: FilterMode, rawInput: string) {
    setError(null);
    setQueryError(null);
    setSessionInfo(null);
    const input = rawInput.trim();
    const key = queueKey(mode, input);
    let filter: ReviewFilter | undefined;
    let info: string | null = null;
    try {
      try {
        if (mode === 'query' && input) {
          if (queryHasLimit(input)) {
            const snapshot = await createReviewSnapshot(input);
            filter = { FilteredTag: { tag_id: snapshot.tag_id } };
            info = `Reviewing \`${input}\` as filtered tag \`${snapshot.tag_name}\` (${snapshot.rebuilt ? 'built for today' : 'resumed from earlier today'}, ${snapshot.card_count} cards left)`;
          } else {
            filter = { Query: input };
          }
        } else if (mode === 'tagName' && input) {
          const tag = await getTagByName(input);
          filter = { FilteredTag: { tag_id: tag.id } };
          info = `Reviewing filtered tag \`${tag.name}\``;
        } else if (mode === 'tagId' && input) {
          const id = Number(input);
          if (!Number.isInteger(id)) throw new Error(`Invalid tag id: ${input}`);
          filter = { FilteredTag: { tag_id: id } };
        }
        // Download the session's cards in case the connection drops
        refreshQueue(key, filter ?? null, info);
      } catch (e) {
        // Offline, use what the filter resolved to when its cards were downloaded
        const queue = isNetworkError(e) ? await getOfflineQueue(key) : null;
        if (!queue) throw e;
        filter = queue.filter ?? undefined;
        info = queue.sessionInfo;
      }
    } catch (e) {
      setError(errorMessage(e));
      if (mode === 'query' && e instanceof ApiError && e.span) {
        setQueryError({ query: input, span: e.span });
        setPhase('landing');
        return;
      }
      setPhase('error');
      return;
    }
    setSessionInfo(info ?? describeFilter(mode, input));
    sessionKey.current = key;
    setSessionKeyShown(key);
    if (input) recentFilters.save({ mode, input });
    setSearchParams(prev => {
      const params = filterParams(mode, input);
      // Find in queue's query stays for when the session ends
      const match = prev.get('match');
      if (match) params.set('match', match);
      return params;
    }, { replace: true });
    setActiveFilter(filter);
    setReviewedCount(0);
    setSessionRecallMs(0);
    setSessionRateMs(0);
    setSessionStart(Date.now());
    setSessionEnd(null);
    setSessionShowedCard(false);
    lastEventId.current = null;
    lastActionWasRating.current = false;
    lastPendingAction.current = null;
    setLastUndo(null);
    loadCard(filter);
  }

  /**
   * Leaves the session for the landing page, which shows its summary above the day's statistics,
   * keeping the current filter prefilled to edit.
   */
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
      const res = await performStudyAction(credentials.schedulerName, {
        Rate: {
          card_id: card.card_id,
          rating: ratingId,
          recall_duration: recallSeconds,
          rate_duration: rateSeconds,
          tag_id: tagId,
        },
      });
      setSessionRateMs(ms => ms + rateMs);
      setReviewedCount(c => c + 1);
      lastEventId.current = res.eventId;
      lastActionWasRating.current = true;
      lastPendingAction.current = pendingAction(res, card);
      setLastUndo(null);
      const ratedCardId = card.card_id;
      setPreviousCard(prev => ({ id: ratedCardId, version: (prev?.version ?? 0) + 1 }));
      // Shortened on phones, where it sits above the card
      setStatus(isNarrow
        ? `Previous card: Recall ${formatDuration(recallSeconds)} · Rate ${formatDuration(rateSeconds)}${res.sent ? '' : ' · Saved offline'}`
        : `Previous card — Recall Duration: ${formatDuration(recallSeconds)} · Rate Duration: ${formatDuration(rateSeconds)}${res.sent ? '' : ' · Saved offline, sent once the server can be reached'}`);
      loadCard(activeFilter);
    } catch (e) {
      setError(errorMessage(e));
      setPhase('error');
    }
  }

  /** Runs an action that removes the current card from today's queue, then advances. */
  async function runCardAction(action: () => Promise<number | null>, message?: string) {
    if (!card) return;
    const cardId = card.card_id;
    try {
      lastPendingAction.current = null;
      lastEventId.current = await action();
      lastActionWasRating.current = false;
      setLastUndo(null);
      setStatus(message ?? null);
      // Not to be shown again from the downloaded queue if the connection drops
      await forgetQueuedCard(cardId);
      loadCard(activeFilter);
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  /** The record of an action that is yet to be sent, for Undo to take back. */
  function pendingAction(res: ActionResult, actedOn: GetReviewCardResponse) {
    return res.sent ? null : { clientId: res.clientId, card: actedOn };
  }

  function buryCard() {
    if (!card || !credentials) return;
    const { schedulerName } = credentials;
    const buried = card;
    runCardAction(async () => {
      const res = await performStudyAction(schedulerName, { Bury: { card_id: buried.card_id } });
      lastPendingAction.current = pendingAction(res, buried);
      return res.eventId;
    }, 'Card buried.');
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

  function setDueDate(target: 'card' | 'note', due: Date, message = 'Due date updated.') {
    if (!card) return;
    runCardAction(async () => {
      let ids = [card.card_id];
      if (target === 'note') {
        // Only push sibling cards later, never pull them earlier
        const cards = await getCardsForNote(card.note_id);
        ids = cards.filter(c => new Date(c.due) <= due).map(c => c.id);
      }
      return (await updateCards({ selector: { Ids: ids }, due: due.toISOString() })).event_id;
    }, message);
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

  /** Pushes the card past the session's bury-until date, e.g. an exam's, asking for it if unset. */
  function buryCardUntilDate() {
    if (!buryUntil) {
      openBuryUntilPicker('bury');
      return;
    }
    buryCardUntil(buryUntil);
  }

  function buryCardUntil(date: string) {
    setDueDate('card', dayAfter(date), `Card buried until after ${formatBuryUntil(date)}.`);
  }

  function openBuryUntilPicker(mode: 'bury' | 'change') {
    setBuryUntilValue(buryUntil ?? '');
    setBuryUntilPicker(mode);
  }

  function saveBuryUntil() {
    if (!buryUntilValue) return;
    setBuryUntil(buryUntilValue);
    setBuryUntilPicker(null);
    if (buryUntilPicker === 'bury') buryCardUntil(buryUntilValue);
  }

  async function flagNote() {
    if (!card || !config) return;
    try {
      lastEventId.current = await tagNote(card.note_id, config.flagged_tag_name);
      lastActionWasRating.current = false;
      setLastUndo(null);
      setStatus(`Note tagged \`${config.flagged_tag_name}\`.`);
    } catch (e) {
      setStatus(errorMessage(e));
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
    const pending = lastPendingAction.current;
    if (pending) {
      lastPendingAction.current = null;
      // Sent since, e.g. when the connection came back. Its event is unknown here.
      if (!(await cancelPendingAction(pending.clientId))) {
        setStatus('It was already sent to the server. Use Undo last change to undo it there.');
        return;
      }
      await returnToQueue(sessionKey.current, pending.card);
      setStatus('Undone before it was sent.');
      if (lastActionWasRating.current) {
        setReviewedCount(c => Math.max(0, c - 1));
        setPreviousCard(null);
      }
      lastActionWasRating.current = false;
      loadCard(activeFilter);
      return;
    }
    try {
      // Use the tracked event id so syncs from elsewhere don't cause the wrong event to be undone
      const res = await undoEvent(lastEventId.current);
      lastEventId.current = null;
      setStatus(res ? describeReverted('Undone', res.undone_events) : 'No event to undo.');
      const undoId = res?.undo_event_ids[0];
      const ratedCardId = lastActionWasRating.current ? previousCard?.id ?? null : null;
      setLastUndo(undoId === undefined ? null : { eventId: undoId, wasRating: lastActionWasRating.current, ratedCardId });
      if (lastActionWasRating.current) {
        setReviewedCount(c => Math.max(0, c - 1));
        // The rated card is back on screen, and the one rated before it isn't tracked
        setPreviousCard(null);
      }
      lastActionWasRating.current = false;
      // The next card will be the one that was just undone
      loadCard(activeFilter);
    } catch (e) {
      setStatus(errorMessage(e));
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
      const redoneCardId = lastUndo.ratedCardId;
      if (redoneCardId !== null) setPreviousCard(prev => ({ id: redoneCardId, version: (prev?.version ?? 0) + 1 }));
      // The redone action moved the card on again
      loadCard(activeFilter);
    } catch (e) {
      setStatus(errorMessage(e));
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
      setStatus(errorMessage(e));
    } finally {
      setNoteLoading(false);
    }
  }

  function toggleCardNote() {
    if (!card) return;
    if (panelNote?.id === card.note_id) setPanelNote(null);
    else openNote(card.note_id);
  }

  // The editor opens below a card that can be taller than the viewport, so bring it into view.
  // Beside the card it is already in view.
  const panelNoteId = panelNote?.id;
  useEffect(() => {
    if (panelNoteId !== undefined && !isWide) notePanelRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, [panelNoteId, isWide]);

  // Load the Typst compiler, its fonts and the renderer while the filter is picked, so the first
  // Typst card isn't delayed by them.
  // Also download today's cards, so a review can start, or continue, without a connection.
  const onLanding = phase === 'landing';
  useEffect(() => {
    if (!onLanding) return;
    import('../typst/compiler').then(({ warmUpTypst }) => warmUpTypst()).catch(e => { if (!isNetworkError(e)) console.error(e); });
    refreshQueue(queueKey('query', ''), null, null);
  }, [onLanding]);

  // Compile the next card downloaded for the filter entered, so that it shows at once if the review
  // starts with it. After a pause in typing, so each key does not start a compile.
  const landingQueueKey = onLanding ? queueKey(filterMode, filterInput) : null;
  useEffect(() => {
    if (landingQueueKey === null) return;
    const timer = setTimeout(() => {
      precompileNextCard(landingQueueKey).catch(e => { if (!isNetworkError(e)) console.error(e); });
    }, PRECOMPILE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [landingQueueKey]);

  // Likewise compile the card likely to come next while this one is shown, once its back has had
  // time to compile first, so rating it shows the next card at once
  const shownCardId = frontRendered ? card?.card_id : undefined;
  useEffect(() => {
    if (shownCardId === undefined) return;
    const timer = setTimeout(() => {
      precompileNextCard(sessionKey.current, shownCardId).catch(e => { if (!isNetworkError(e)) console.error(e); });
    }, PRECOMPILE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [shownCardId]);

  // Load the note editor while a card is shown, so opening the note isn't delayed by it
  const reviewing = phase === 'front' || phase === 'back';
  useEffect(() => {
    if (reviewing) loadNoteDetail().catch(console.error);
  }, [reviewing]);
  // Likewise the Vim answer box, for cards with typed answers
  const vimKeybindings = useVimKeybindings();
  useEffect(() => {
    if (reviewing && vimKeybindings) import('../components/VimAnswerInput').catch(console.error);
  }, [reviewing, vimKeybindings]);

  // The page leaves room below the card for the pinned answer bar, which grows with the typed
  // answer boxes
  const hasAnswerBar = card !== null && ((phase === 'front' && !card.cli) || phase === 'back');
  useEffect(() => {
    const bar = answerBarRef.current;
    if (!hasAnswerBar || !bar) return;
    const root = document.documentElement;
    const observer = new ResizeObserver(() => root.style.setProperty('--answer-bar-height', `${bar.offsetHeight}px`));
    observer.observe(bar);
    return () => {
      observer.disconnect();
      root.style.removeProperty('--answer-bar-height');
    };
  }, [hasAnswerBar]);

  // After leaving a session, focus the prefilled filter so a new one can be typed straight away
  const returnedToLanding = phase === 'landing' && sessionStart !== null;
  useEffect(() => {
    if (returnedToLanding) filterInputRef.current?.select();
  }, [returnedToLanding]);

  /**
   * Fetches the current card again and shows it in place, on the side shown, without restarting
   * its stopwatch. For after its note changed, here or by `spares import` of its file.
   */
  async function refreshCurrentCard(refreshed: string, deleted: string) {
    if (!card) return;
    try {
      const next = await getReviewCardById(card.card_id, activeFilter);
      if (next) {
        setCard(next);
        setCardVersion(v => v + 1);
        setStatus(refreshed);
      } else {
        setStatus(deleted);
        loadCard(activeFilter);
      }
    } catch (e) {
      setStatus(`Failed to refresh the card: ${errorMessage(e)}`);
    }
  }

  /** Mirrors the CLI's Sync Note: re-render the note, then refresh the current card in place. */
  function onNoteUpdated(updated: NoteResponse) {
    setPanelNote(prev => keepShown(prev, updated));
    if (!card || updated.id !== card.note_id) return;
    // NoteDetail has already regenerated the note's rendered files
    refreshCurrentCard(
      'Current card refreshed after sync (card order may have changed).',
      'Current card was deleted during sync. Advancing to next card.',
    );
  }

  /** Shows the card as it is now, e.g. after its file was edited and imported again. */
  function reloadCard() {
    refreshCurrentCard('Card reloaded.', 'The card no longer exists. Advancing to next card.');
  }

  // Coming back to the page, e.g. from the terminal after `spares import` of the card's file,
  // refreshes the card if its note changed meanwhile. Focus and becoming visible often come
  // together, so a check already under way is not repeated. Re-bound on every render so the card
  // is current.
  const checkingNote = useRef(false);
  useEffect(() => {
    if (!reviewing || !card || offline) return;
    const shown = card;
    async function checkNote() {
      if (document.visibilityState !== 'visible' || checkingNote.current) return;
      checkingNote.current = true;
      try {
        if (noteChangedSince(shown, await getNote(shown.note_id))) {
          await refreshCurrentCard(
            'Card refreshed, since its note changed (card order may have changed).',
            'The card was deleted when its note changed. Advancing to next card.',
          );
        }
      } catch (e) {
        if (!isNetworkError(e)) console.error(e);
      } finally {
        checkingNote.current = false;
      }
    }
    window.addEventListener('focus', checkNote);
    document.addEventListener('visibilitychange', checkNote);
    return () => {
      window.removeEventListener('focus', checkNote);
      document.removeEventListener('visibilitychange', checkNote);
    };
  });

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
      setStatus(errorMessage(e));
    }
  }

  // Keyboard shortcuts, as listed by the help popup, which ? opens
  const [showKeybindings, setShowKeybindings] = useState(false);
  const cardShown = reviewing && card !== null;
  const hasAudio = !!card?.audio && (card.audio.front.length > 0 || (phase === 'back' && card.audio.back.length > 0));
  const pageWidth = usePageWidth();
  function cyclePageWidth() {
    setPageWidth(PAGE_WIDTHS[(PAGE_WIDTHS.indexOf(pageWidth) + 1) % PAGE_WIDTHS.length]);
  }
  const cardZoom = useCardZoom();
  const zoomIndex = CARD_ZOOMS.indexOf(cardZoom);
  const zoomIn = () => setCardZoom(CARD_ZOOMS[Math.min(zoomIndex + 1, CARD_ZOOMS.length - 1)]);
  const zoomOut = () => setCardZoom(CARD_ZOOMS[Math.max(zoomIndex - 1, 0)]);
  const pagesPerRow = usePagesPerRow();
  const keybindings: Keybinding[] = [
    { keys: ['Space'], description: 'Show answer', available: phase === 'front' },
    ...ratings.map((r, i) => ({ keys: [String(i + 1)], description: `Rate ${r.description}`, available: phase === 'back' })),
    { keys: ['u'], description: phase === 'back' ? 'Undo (hide the answer)' : 'Undo', available: reviewing },
    { keys: ['U'], description: 'Redo', available: reviewing && lastUndo !== null },
    { keys: ['b'], description: 'Bury card', available: cardShown },
    { keys: ['B'], description: 'Bury note (card + siblings)', available: cardShown },
    { keys: ['l'], description: 'Bury card until later today', available: cardShown },
    { keys: ['d'], description: buryUntil ? `Bury card until after ${formatBuryUntil(buryUntil)}` : 'Bury card until after a date, e.g. an exam', available: cardShown },
    { keys: ['s'], description: 'Suspend card', available: cardShown },
    { keys: ['e'], description: cardShown && panelNote?.id === card.note_id ? 'Close note' : 'Edit note', available: cardShown },
    { keys: ['R'], description: 'Reload card, e.g. after importing its file again', available: cardShown },
    { keys: ['f'], description: `Tag note to modify later${config ? ` (${config.flagged_tag_name})` : ''}`, available: cardShown && config !== null },
    { keys: ['q'], description: 'Switch query', available: reviewing },
    { keys: ['Q'], description: showFindInQueue ? 'Close Find in queue' : "Find cards in this session's queue", available: reviewing },
    { keys: ['r'], description: 'Replay audio', available: reviewing && hasAudio },
    { keys: ['w'], description: `Change the page width (now ${PAGE_WIDTH_LABELS[pageWidth]})`, available: reviewing && !isNarrow },
    { keys: ['+', '='], description: `Larger card text (now ${cardZoom}%)`, available: reviewing },
    { keys: ['-'], description: 'Smaller card text', available: reviewing },
    ...TYPED_ANSWER_SHORTCUTS.map(s => ({ ...s, available: phase === 'front' && !!card?.typed_answer })),
  ];

  // Re-bound on every render so closures are current
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      const target = e.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) return;
      if (target instanceof Element && target.closest('.cm-editor')) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === '?') {
        e.preventDefault();
        setShowKeybindings(v => !v);
        return;
      }
      // The popup covers the card, so nothing should change behind it
      if (showKeybindings) return;
      if (phase !== 'front' && phase !== 'back') return;
      // Space shows the answer, so it must not also click a button (or toggle a section) left
      // focused by a mouse click, e.g. Bury Card when pressed on the back
      if (e.code === 'Space' && target instanceof Element && target.closest('button, summary')) e.preventDefault();
      const key = e.code === 'Space' ? 'Space' : e.key;
      if (!keybindings.some(b => b.available && b.keys.includes(key))) return;
      const actions: Record<string, () => void> = {
        Space: showAnswer, u: undo, U: redo, b: buryCard, B: buryNote, l: buryUntilLaterToday, d: buryCardUntilDate, s: suspendCard, e: toggleCardNote,
        R: reloadCard, f: flagNote, q: switchQuery, Q: () => setShowFindInQueue(v => !v), r: replayAudio, w: cyclePageWidth,
        '+': zoomIn, '=': zoomIn, '-': zoomOut,
      };
      ratings.forEach((r, i) => { actions[String(i + 1)] = () => rate(r.id); });
      const action = actions[key];
      if (!action) return;
      if (key === 'Space') e.preventDefault();
      action();
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

  const sessionButton = toned(TONES.gray, isNarrow ? { padding: '4px 8px', fontSize: 12 } : actionButton);
  const dueInLabel = config ? formatDuration(config.set_card_due_date_duration) : '…';
  const sessionStarted = sessionStart !== null;

  // A stat in the summary, styled like the due counts' cards on the landing page
  const summaryStat = (value: React.ReactNode, label: string) => (
    <div key={label} style={{ minWidth: 64 }}>
      <div style={{ fontSize: 20, fontWeight: 600, lineHeight: '26px', color: 'var(--text)' }}>{value}</div>
      <div style={{ fontSize: 12, lineHeight: '16px', color: 'var(--text-secondary)' }}>{label}</div>
    </div>
  );
  const summaryGroup = (label: string, stats: React.ReactNode[]) => (
    <div>
      <div style={sectionLabel}>{label}</div>
      <div style={{ display: 'flex', gap: isNarrow ? 16 : 24, flexWrap: 'wrap' }}>{stats}</div>
    </div>
  );
  const summary = sessionStarted && (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: isNarrow ? '12px 14px' : '12px 20px', textAlign: 'left', display: 'inline-flex', gap: isNarrow ? 16 : 40, flexWrap: 'wrap' }}>
      {summaryGroup('Session', [
        summaryStat(reviewedCount, 'Cards reviewed'),
        summaryStat(formatDuration(msToSeconds((sessionEnd ?? sessionStart ?? 0) - (sessionStart ?? 0))), 'Total time'),
        summaryStat(formatDuration(msToSeconds(sessionRecallMs)), 'Recall time'),
        summaryStat(formatDuration(msToSeconds(sessionRateMs)), 'Rate time'),
      ])}
      {statistics && summaryGroup('Today', [
        summaryStat(statistics.cards_studied_count, 'Cards studied'),
        summaryStat(formatDuration(statistics.recall_duration), 'Recall time'),
        summaryStat(formatDuration(statistics.rate_duration), 'Rate time'),
        ...(statistics.cram_count > 0
          ? [summaryStat(`${statistics.cram_count} (${formatDuration(statistics.cram_duration)})`, 'Crams')]
          : []),
      ])}
    </div>
  );

  // On wide screens the editor takes the right half of the screen, and the page the left
  const splitNote = isWide && panelNote !== null;
  const notePanel = panelNote && (
    <div ref={notePanelRef} className={splitNote ? 'review-note-split' : undefined} style={splitNote ? undefined : { marginTop: 16, scrollMarginTop: 16 }}>
      <Suspense fallback={<div style={{ fontSize: 13, color: 'var(--text-muted)' }}>Loading note…</div>}>
        <NoteDetail
          key={panelNote.id}
          note={panelNote}
          onClose={() => setPanelNote(null)}
          onNoteUpdated={onNoteUpdated}
        />
      </Suspense>
    </div>
  );

  return (
    <div className={splitNote ? 'page review-split' : 'page'}>
      <Navbar onLogout={logout} help={{ open: showKeybindings, onOpenChange: setShowKeybindings, bindings: keybindings }} />
      {/* On phones the heading, its top margin and the session's actions are kept to one short row,
          so the card starts near the top of the screen */}
      <div style={{ display: 'flex', alignItems: isNarrow ? 'center' : 'baseline', gap: 8, marginBottom: isNarrow ? 8 : 0 }}>
        <h2 style={isNarrow ? { margin: 0 } : { marginBottom: 16 }}>Review</h2>
        {/* The session's actions, out of the way of the card's at the top right */}
        {cardShown && (
          <div style={{ marginLeft: 'auto', marginBottom: isNarrow ? 0 : 12, display: 'flex', gap: isNarrow ? 6 : 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>
            <button onClick={switchQuery} className="touch-target-small" style={sessionButton}>{isNarrow ? 'Switch' : 'Switch query'} <span className="key-hint">(q)</span></button>
            <button onClick={() => setShowPreviousCardInfo(v => !v)} disabled={!previousCard} className="touch-target-small" style={sessionButton}>
              {showPreviousCardInfo && previousCard ? 'Hide ' : ''}{isNarrow ? 'Previous' : 'Previous Card Info'}
            </button>
            <button onClick={switchQuery} className="touch-target-small" style={sessionButton}>{isNarrow ? 'End' : 'End session'}</button>
          </div>
        )}
      </div>

      {phase === 'landing' && (
        <div style={{ marginTop: sessionStarted && sessionShowedCard ? 0 : 24 }}>
          {sessionStarted && sessionShowedCard && <div style={{ marginBottom: 20 }}>{summary}</div>}
          {/* Rendered before the statistics load, at the height of one row of cards, so the search bar below does not shift.
              On phones the four states' cards are narrowed to fit on that one row. */}
          <div style={{ display: 'flex', gap: isNarrow ? 8 : 12, marginBottom: 20, flexWrap: isNarrow ? 'nowrap' : 'wrap', alignItems: 'center', minHeight: DUE_CARD_HEIGHT }}>
            {statistics && Object.entries(statistics.due_count_by_state)
              .filter(([, count]) => count > 0)
              .map(([stateId, count]) => (
                <div key={stateId} style={{ border: '1px solid var(--border)', borderRadius: 6, padding: isNarrow ? '6px 8px' : '6px 14px', textAlign: 'center', boxSizing: 'border-box', height: DUE_CARD_HEIGHT, minWidth: 0 }}>
                  <div style={{ fontSize: 20, fontWeight: 600, lineHeight: '26px' }}>{count}</div>
                  <div style={{ fontSize: 12, lineHeight: '16px', color: 'var(--text-secondary)' }}>{STATE_LABELS[Number(stateId)] ?? stateId}</div>
                </div>
              ))}
            {statistics && Object.values(statistics.due_count_by_state).every(c => c === 0) && (
              <p style={{ color: 'var(--text-secondary)', margin: 0 }}>Nothing due to review.</p>
            )}
          </div>
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
            {filterMode === 'query' ? (
              <SearchInput
                ref={filterInputRef}
                placeholder={FILTER_MODES.find(m => m.mode === filterMode)?.placeholder}
                value={filterInput}
                onChange={setFilterInput}
                onSubmit={input => startReview(filterMode, input)}
                style={{ maxWidth: 'none' }}
                inputStyle={filterInputStyle}
                error={queryError}
              />
            ) : (
              <input
                ref={filterInputRef}
                type={filterMode === 'tagId' ? 'number' : 'text'}
                placeholder={FILTER_MODES.find(m => m.mode === filterMode)?.placeholder}
                value={filterInput}
                onChange={e => setFilterInput(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') startReview(filterMode, filterInput); }}
                style={{ flex: 1, ...filterInputStyle }}
              />
            )}
            <button onClick={() => startReview(filterMode, filterInput)} className="touch-target" style={{ ...primaryButton, padding: '8px 24px', fontSize: 14 }}>
              Start Review
            </button>
            <button onClick={() => setShowQueue(v => !v)} aria-pressed={showQueue} className="touch-target" style={{ padding: '8px 16px', fontSize: 14 }} title="The queue of this filter, and where cards are in it">
              Find in Queue
            </button>
          </div>
          {filterMode === 'query' && queryError && error && (
            <pre style={{ color: 'var(--error)', marginTop: 0, marginBottom: 8, whiteSpace: 'pre-wrap' }}>{error}</pre>
          )}
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
          {showQueue && <FindInQueue filter={{ mode: filterMode, input: filterInput }} onOpenNote={openNote} onClose={() => setShowQueue(false)} />}
          {noteLoading && <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>Loading note…</div>}
          {notePanel}
          {history && (
            <div style={{ marginTop: 40 }}>
              <ReviewHeatmap counts={history.review_count_by_date} cramCounts={history.cram_count_by_date} />
            </div>
          )}
        </div>
      )}

      {phase === 'loading' && <div className="loading-delayed">Loading…</div>}

      {phase === 'waiting' && waitingUntil !== null && (
        <div style={{ textAlign: 'center', marginTop: 80, color: 'var(--text-secondary)' }}>
          <p>
            Learning cards are due later today. The next can be shown at{' '}
            {new Date(waitingUntil).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
            {' '}(in {formatWait(waitingUntil - Date.now())}), and will be once it can.
          </p>
          <p style={{ marginTop: 24, display: 'flex', gap: 16, justifyContent: 'center' }}>
            <button onClick={switchQuery}>End session</button>
          </p>
        </div>
      )}

      {phase === 'done' && (
        <div style={{ textAlign: 'center', marginTop: 80, color: 'var(--text-secondary)' }}>
          <p>Nothing left to review{offline ? ' of the cards downloaded for offline review' : ''}.</p>
          {status && <p style={{ fontSize: 13 }}>{status}</p>}
          {summary && <div style={{ marginTop: 24 }}>{summary}</div>}
          <p style={{ marginTop: 24, display: 'flex', gap: 16, justifyContent: 'center' }}>
            <button onClick={() => { setStatus(null); setPhase('landing'); }}>New session</button>
            <Link to="/notes">View notes</Link>
          </p>
        </div>
      )}

      {phase === 'error' && (
        <div>
          {/* Kept as written, since a query error marks the problem on the line below the query */}
          <pre style={{ color: 'var(--error)', marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>
          <button onClick={() => (card || sessionStarted ? loadCard(activeFilter) : setPhase('landing'))}>Retry</button>
          <button onClick={() => setPhase('landing')} style={{ marginLeft: 8 }}>Back</button>
        </div>
      )}

      {(phase === 'front' || phase === 'back') && card && (
        <>
          {/* What happened to the previous card, above the card so it is seen without scrolling.
              On phones it is one line, which a tap unfolds, so the card is not pushed down. */}
          {status && (
            <div
              onClick={isNarrow ? () => setExpandedStatus(statusExpanded ? null : status) : undefined}
              title={isNarrow && !statusExpanded ? status : undefined}
              style={{
                marginBottom: 8, fontSize: isNarrow ? 12 : 13, color: 'var(--text-secondary)',
                ...(isNarrow && !statusExpanded && { whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }),
              }}
            >
              {status}
            </div>
          )}

          {/* Scrolls within a capped height, so the card stays in view below a long history */}
          {showPreviousCardInfo && previousCard && (
            <div style={{ marginBottom: 12, padding: isNarrow ? 8 : 12, border: '1px solid var(--border)', borderRadius: 6, maxHeight: isNarrow ? '35vh' : '45vh', overflowY: 'auto' }}>
              <div style={sectionLabel}>Previous Card Info (card {previousCard.id})</div>
              <CardInfo cardId={previousCard.id} version={previousCard.version} />
            </div>
          )}

          {showFindInQueue && (
            <FindInQueue
              filter={{ resolved: activeFilter ?? null }}
              onOpenNote={openNote}
              onClose={() => setShowFindInQueue(false)}
              refreshKey={card}
              currentCardId={card?.card_id}
              autoFocus
              style={{ marginTop: 0, marginBottom: 12, padding: isNarrow ? 8 : 12, maxHeight: isNarrow ? '50vh' : '60vh', overflowY: 'auto' }}
            />
          )}

          {sessionInfo && <div style={{ marginBottom: 8, fontSize: 13, color: 'var(--text-secondary)', overflowWrap: 'anywhere' }}><CodeText text={sessionInfo} /></div>}
          <div style={{ marginBottom: 8, fontSize: 13, color: 'var(--text-muted)', display: 'flex', gap: 8, flexWrap: 'wrap' }}>
            <span>Note {formatNoteId(card.note_id)} · Card {card.card_id} (#{card.card_order}) · {card.parser_name}</span>
            {offline && <span style={{ color: 'var(--warning)' }} title="The server cannot be reached. Ratings are saved and sent once it can.">Offline</span>}
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
                  This card is reviewed by running <code>{card.cli.exec}</code>, which only the CLI can do. Bury it or review it in the terminal with:
                </p>
                {/* `user-select: all` so a click or double-click selects the whole command, not one word */}
                <pre style={{ margin: '8px 0 0', padding: '6px 10px', background: 'var(--surface)', borderRadius: 4, overflowX: 'auto', userSelect: 'all' }}>
                  <code>spares card review -q c.id={card.card_id}</code>
                </pre>
              </div>
            ) : (
              <>
                {phase === 'front' && card.audio && <AudioBar audio={card.audio} flipped={false} player={audioPlayer} />}
                <div onClick={onFrontClick} style={{ border: '1px solid var(--border)', borderRadius: 4, overflow: 'hidden', marginBottom: 16, cursor: phase === 'front' ? 'pointer' : undefined }}>
                  <CardRenderer path={card.card_front_rendered_path} source={card.browser_sources?.card_front} webPath={card.card_front_web_path} version={cardVersion} onOpenNote={openNote} onReady={onFrontRendered} onPlayAudio={playAudioClip} onTypstWidth={rememberReviewWidth} scrollToCloze={phase === 'front'} />
                </div>
              </>
            )}
          </div>

          {(phase === 'back' || (frontRendered && !card.cli)) && (
            <div style={phase === 'back' ? undefined : hiddenSide} aria-hidden={phase !== 'back'}>
              {phase === 'back' && card.audio && <AudioBar audio={card.audio} flipped player={audioPlayer} />}
              {phase === 'back' && card.typed_answer && <TypedAnswerResult info={card.typed_answer} answers={typedAnswers} />}
              <div style={{ border: '1px solid var(--border)', borderRadius: 4, overflow: 'hidden', marginBottom: 16, background: 'var(--surface)' }}>
                <CardRenderer path={backPath(card)} source={card.browser_sources?.card_back} webPath={backWebPath(card)} version={cardVersion} onOpenNote={openNote} onPlayAudio={playAudioClip} scrollToCloze={phase === 'back'} />
              </div>
            </div>
          )}

          {((phase === 'front' && !card.cli) || phase === 'back') && (
            <div ref={answerBarRef} className="review-answer-bar">
              {/* Pinned within thumb's reach on phones, rather than below a card that may be long */}
              {isNarrow && (
                <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
                  <button onClick={buryCard} className="touch-target-small" style={toned(TONES.orange, { ...actionButton, flex: 1 })}>Bury Card</button>
                  <button onClick={buryNote} className="touch-target-small" style={toned(TONES.orange, { ...actionButton, flex: 1 })}>Bury Note</button>
                  {/* Only once a date is picked, so the row stays short when the session has none */}
                  {buryUntil && (
                    <button onClick={buryCardUntilDate} className="touch-target-small" style={toned(TONES.orange, { ...actionButton, flex: 1 })}>
                      Until {formatBuryUntil(buryUntil)}
                    </button>
                  )}
                </div>
              )}
              {phase === 'front' ? (
                <>
                  {/* Pinned with the button, so a card that wants a typed answer shows it however
                      far the card is scrolled. Unmounted on the back, so no hidden box keeps the
                      focus from the rating hotkeys. */}
                  {card.typed_answer && (
                    <TypedAnswerInputs info={card.typed_answer} answers={typedAnswers} onChange={setTypedAnswers} onSubmit={showAnswer} />
                  )}
                  <button onClick={showAnswer} className="touch-target" style={{ ...primaryButton, width: '100%', padding: 12, fontSize: 15, fontWeight: 600 }}>
                    {card.typed_answer ? 'Check Answer' : 'Show Answer'}{' '}
                    <span className="key-hint" style={{ fontSize: 12, fontWeight: 400 }}>({card.typed_answer ? 'Enter' : 'Space'})</span>
                  </button>
                </>
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

          {noteLoading && <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>Loading note…</div>}
          {notePanel}

          <div style={{ marginTop: 20, display: 'flex', flexDirection: 'column', gap: 14 }}>
            {isNarrow && (
              <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                <button onClick={undo} className="touch-target" style={toned(TONES.gray)}><UndoIcon />Undo</button>
                {lastUndo && <button onClick={redo} className="touch-target" style={toned(TONES.gray)}><RedoIcon />Redo</button>}
                <button onClick={toggleCardNote} className="touch-target" style={toned(TONES.blue)}>{panelNote?.id === card.note_id ? 'Close Note' : 'Edit Note'}</button>
                {/* In the answer bar when there is one */}
                {!hasAnswerBar && <button onClick={buryCard} className="touch-target" style={toned(TONES.orange)}>Bury Card</button>}
                {!hasAnswerBar && <button onClick={buryNote} className="touch-target" style={toned(TONES.orange)}>Bury Note</button>}
                {!hasAnswerBar && buryUntil && <button onClick={buryCardUntilDate} className="touch-target" style={toned(TONES.orange)}>Bury Until {formatBuryUntil(buryUntil)}</button>}
                <button onClick={suspendCard} className="touch-target" style={toned(TONES.purple)}>Suspend Card</button>
              </div>
            )}
            {/* Remount when the width class changes so it opens by default on wide screens */}
            <details key={String(isNarrow)} open={!isNarrow} className="review-more-actions">
              <summary>More actions</summary>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: isNarrow ? 8 : 0 }}>
                <ActionGroup label="Note" tone={TONES.blue}>
                  {!isNarrow && <button onClick={undo} style={toned(TONES.gray)}><UndoIcon />Undo <span className="key-hint">(u)</span></button>}
                  {!isNarrow && lastUndo && <button onClick={redo} style={toned(TONES.gray)}><RedoIcon />Redo <span className="key-hint">(U)</span></button>}
                  {!isNarrow && <button onClick={toggleCardNote} style={toned(TONES.blue)}>{panelNote?.id === card.note_id ? 'Close Note' : 'Edit Note'} <span className="key-hint">(e)</span></button>}
                  <button onClick={flagNote} disabled={!config} style={toned(TONES.blue)}>
                    Tag to modify later{config ? ` (${config.flagged_tag_name})` : ''} <span className="key-hint">(f)</span>
                  </button>
                  <button onClick={reloadCard} style={toned(TONES.blue)} title="Shows the card as it is now, e.g. after its file was edited and imported again">
                    Reload Card <span className="key-hint">(R)</span>
                  </button>
                </ActionGroup>
                <ActionGroup label="Queue" tone={TONES.gray}>
                  <button onClick={() => setShowFindInQueue(v => !v)} aria-pressed={showFindInQueue} style={toned(TONES.gray)} title="Where cards are in this session's queue, and why others are not in it">
                    {showFindInQueue ? 'Close Find in Queue' : 'Find in Queue'} <span className="key-hint">(Q)</span>
                  </button>
                </ActionGroup>
                <ActionGroup label="Remove from today's queue" tone={TONES.orange}>
                  {!isNarrow && <button onClick={buryCard} style={toned(TONES.orange)}>Bury Card <span className="key-hint">(b)</span></button>}
                  <button onClick={buryNote} style={toned(TONES.orange)}>Bury Note (card + siblings) <span className="key-hint">(B)</span></button>
                  <button onClick={buryUntilLaterToday} style={toned(TONES.orange)}>Bury Until Later Today <span className="key-hint">(l)</span></button>
                  {/* On phones, once a date is picked, it is with Bury Card instead */}
                  {(!isNarrow || !buryUntil) && (
                    <button onClick={buryCardUntilDate} style={toned(TONES.orange)}>
                      {buryUntil ? `Bury Card Until After ${formatBuryUntil(buryUntil)}` : 'Bury Card Until After…'} <span className="key-hint">(d)</span>
                    </button>
                  )}
                  {buryUntil && <button onClick={() => openBuryUntilPicker('change')} style={toned(TONES.gray)}>Change Date</button>}
                  {buryUntilPicker && (
                    <form
                      onSubmit={e => { e.preventDefault(); saveBuryUntil(); }}
                      style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', fontSize: 13, flexBasis: '100%' }}
                    >
                      <span>Bury cards until after (e.g. the exam), remembered for this query or tag:</span>
                      <input type="date" value={buryUntilValue} min={localDateString()} onChange={e => setBuryUntilValue(e.target.value)} autoFocus />
                      <button type="submit" disabled={!buryUntilValue} style={toned(TONES.orange)}>{buryUntilPicker === 'bury' ? 'Bury' : 'Save'}</button>
                      {buryUntilPicker === 'change' && <button type="button" onClick={() => { setBuryUntil(null); setBuryUntilPicker(null); }} style={toned(TONES.gray)}>Clear</button>}
                      <button type="button" onClick={() => setBuryUntilPicker(null)} style={toned(TONES.gray)}>Cancel</button>
                    </form>
                  )}
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
                <ActionGroup label="Display" tone={TONES.gray}>
                  {/* Phones use their whole width already */}
                  {!isNarrow && (
                    <>
                      <span style={{ fontSize: 13 }}>Width <span className="key-hint">(w)</span></span>
                      {PAGE_WIDTHS.map(width => (
                        <button key={width} onClick={() => setPageWidth(width)} aria-pressed={pageWidth === width} style={actionButton}>{PAGE_WIDTH_LABELS[width]}</button>
                      ))}
                      <span style={{ fontSize: 13, marginLeft: 8 }}>Pages per row</span>
                      {PAGES_PER_ROW.map(pages => (
                        <button key={pages} onClick={() => setPagesPerRow(pages)} aria-pressed={pagesPerRow === pages} style={actionButton}>{PAGES_PER_ROW_LABELS[pages]}</button>
                      ))}
                    </>
                  )}
                  <span style={{ fontSize: 13, marginLeft: isNarrow ? 0 : 8 }}>Text size</span>
                  <button onClick={zoomOut} disabled={zoomIndex === 0} style={actionButton} title="Smaller card text (-)">A−</button>
                  <span style={{ fontSize: 13, minWidth: '4ch', textAlign: 'center', fontVariantNumeric: 'tabular-nums' }}>{cardZoom}%</span>
                  <button onClick={zoomIn} disabled={zoomIndex === CARD_ZOOMS.length - 1} style={actionButton} title="Larger card text (+)">A+</button>
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
                      {ln.searched_keyword} ({formatNoteId(ln.note_id)})
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
