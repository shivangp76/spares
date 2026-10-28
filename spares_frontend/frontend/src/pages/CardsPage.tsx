import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { ApiError, cachedCardsPage, countCards, forgetCard, getCard, getQueuePositions, isNetworkError, listCards, listManualTagNames, modifyNoteTags, searchCards, searchCardsPage, updateCards } from '../api/client';
import BulkActionBar, { Checkbox, type BulkAction, type BulkActionBarHandle } from '../components/BulkActionBar';
import { FILTERED_TAG_ACTION } from '../components/bulkActions';
import CardDetail, { type CardDetailHandle } from '../components/CardDetail';
import CardTools from '../components/CardTools';
import { NavbarHelp } from '../components/Navbar';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import Pagination from '../components/Pagination';
import { QueueOrder } from '../components/QueuePositions';
import { setCardColumns, setCardsPageSize, useCardColumns, useCardsPageSize, type CardColumn, type PageSize } from '../preferences';
import { ColumnHeader, ColumnPicker, type Column } from '../components/tableColumns';
import { useTableSort } from '../hooks/useTableSort';
import { useAuth } from '../hooks/useAuth';
import { useCardStates } from '../hooks/useCardStates';
import { useDetailView } from '../hooks/useDetailView';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useRowSelection } from '../hooks/useRowSelection';
import { useListKeys } from '../hooks/useListKeys';
import { useSavedSearches } from '../hooks/useSavedSearches';
import { selectedCount, type IdsOrQuery } from '../selection';
import type { CardMatch, CardResponse, UpdateCardsRequest } from '../types/spares';
import { errorMessage, formatDate, formatDateTime, formatNoteId, tagList, td, th, withPage, withSearch } from '../utils';

const checkTd: React.CSSProperties = { ...td, width: 1, paddingRight: 0 };

/** The ids of the cards `selector` picks. */
async function cardIdsOf(selector: IdsOrQuery): Promise<number[]> {
  return 'Ids' in selector ? selector.Ids : (await searchCards(selector.Query)).map(c => c.id);
}

/** The ids of the notes of the cards `selector` picks. */
async function noteIdsOf(selector: IdsOrQuery): Promise<number[]> {
  const cards = 'Ids' in selector ? await Promise.all(selector.Ids.map(getCard)) : await searchCards(selector.Query);
  return [...new Set(cards.map(c => c.note_id))];
}

/** Sets what `change` gives on the cards `selector` picks, as `spares card edit`. */
async function editCards(selector: IdsOrQuery, change: Omit<UpdateCardsRequest, 'selector'>, done: string) {
  const res = await updateCards({ selector, ...change });
  return { message: `${done} ${res.cards.length} card(s).`, eventIds: res.event_id === null ? [] : [res.event_id] };
}

/** Adds or removes tags of the notes of the cards `selector` picks. */
async function tagNotesOf(selector: IdsOrQuery, add: string[], remove: string[]) {
  const { notes, eventId } = await modifyNoteTags({ Ids: await noteIdsOf(selector) }, add, remove);
  const message = add.length ? `Added ${tagList(add)} to ${notes.length} note(s).` : `Removed ${tagList(remove)} from ${notes.length} note(s).`;
  return { message, eventIds: eventId === null ? [] : [eventId] };
}

const BULK_ACTIONS: BulkAction[] = [
  {
    label: 'Suspend',
    confirm: count => `Suspend ${count} card(s)?`,
    run: selector => editCards(selector, { special_state: 'Suspended' }, 'Suspended'),
  },
  {
    label: 'Bury',
    confirm: count => `Bury ${count} card(s)?`,
    run: selector => editCards(selector, { special_state: 'Buried' }, 'Buried'),
  },
  {
    label: 'Unsuspend',
    confirm: count => `Unsuspend and unbury ${count} card(s)?`,
    run: selector => editCards(selector, { special_state: null }, 'Unsuspended and unburied'),
  },
  {
    label: 'Set due',
    input: { kind: 'datetime' },
    confirm: (count, [due]) => `Make ${count} card(s) due ${new Date(due).toLocaleString()}?`,
    run: (selector, [due]) => editCards(selector, { due: new Date(due).toISOString() }, 'Rescheduled'),
  },
  {
    label: 'Set retention',
    input: { kind: 'number', placeholder: 'e.g. 0.9', min: 0.01, max: 0.99, step: 0.01 },
    confirm: (count, [retention]) => `Set the desired retention of ${count} card(s) to ${retention}?`,
    run: (selector, [retention]) => editCards(selector, { desired_retention: Number(retention) }, 'Set the desired retention of'),
  },
  {
    label: 'Forget',
    danger: true,
    confirm: count => `Forget ${count} card(s)? Their scheduling is reset; review logs are kept.`,
    run: async selector => {
      const ids = await cardIdsOf(selector);
      const eventIds: number[] = [];
      for (const id of ids) {
        const res = await forgetCard(id);
        if (res.event_id !== null) eventIds.push(res.event_id);
      }
      return { message: `Forgot ${ids.length} card(s).`, eventIds };
    },
  },
  {
    label: 'Add tags to notes',
    input: { kind: 'tags', suggestions: listManualTagNames },
    confirm: (count, tags) => `Add ${tagList(tags)} to the notes of ${count} card(s)?`,
    run: (selector, tags) => tagNotesOf(selector, tags, []),
  },
  {
    label: 'Remove tags from notes',
    input: { kind: 'tags', suggestions: listManualTagNames },
    confirm: (count, tags) => `Remove ${tagList(tags)} from the notes of ${count} card(s)?`,
    run: (selector, tags) => tagNotesOf(selector, [], tags),
  },
  FILTERED_TAG_ACTION,
];

/** What a search returns of a card besides the card. */
type CardExtras = Omit<CardMatch, 'card'>;

/** Where a card is in the default review's queue: its place, or held back until later today. */
type QueueSpot = number | 'later';

/**
 * A row of the cards table: a card, with its note's text and what its reviews say when the page
 * was read from a search, and where it is in the queue once that is looked up.
 */
interface CardRow {
  card: CardResponse;
  extras: CardExtras | undefined;
  /** `null` when it isn't in the queue */
  queue: QueueSpot | null | undefined;
}

/** `days` as a short length of time, like `10m`, `5h`, `12d` or `1.5y`. */
function formatDays(days: number): string {
  if (days < 1 / 24) return `${Math.round(days * 24 * 60)}m`;
  if (days < 1) return `${Math.round(days * 24)}h`;
  if (days < 365) return `${Math.round(days)}d`;
  return `${(days / 365).toFixed(1)}y`;
}

/** The columns read from a card's note and reviews, which only a search returns. */
const SEARCH_COLUMNS: readonly CardColumn[] = ['text', 'tags', 'retrievability', 'interval', 'reps', 'lapses', 'leech', 'reviewed'];

const textTd: React.CSSProperties = { maxWidth: 300, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };

function cardColumns(stateName: (state: number) => string): Column<CardRow, CardColumn>[] {
  const nowrap = { whiteSpace: 'nowrap' } as const;
  return [
    { id: 'id', label: 'ID', sortField: 'c.id', render: r => r.card.id },
    { id: 'note', label: 'Note', sortField: 'id', render: r => formatNoteId(r.card.note_id) },
    { id: 'text', label: 'Note text', sortField: 'data', tdStyle: textTd, render: r => <span title={r.extras?.note_preview}>{r.extras?.note_preview ?? '—'}</span> },
    { id: 'tags', label: 'Tags', sortField: 'tag', hideNarrow: true, render: r => r.extras?.note_tags.join(', ') || '—' },
    { id: 'order', label: 'Order', hideNarrow: true, render: r => r.card.order },
    { id: 'state', label: 'State', sortField: 'c.state', render: r => stateName(r.card.state) },
    { id: 'due', label: 'Due', sortField: 'c.due', tdStyle: nowrap, render: r => formatDate(r.card.due) },
    {
      id: 'queue', label: 'Queue', hideNarrow: true,
      render: r => (r.queue === 'later' ? <span title="A learning card, which review shows later today">Later</span> : r.queue === null ? <span title="Not in the default review today">—</span> : r.queue),
    },
    { id: 'retention', label: 'Retention', sortField: 'c.desired_retention', hideNarrow: true, render: r => r.card.desired_retention },
    { id: 'special', label: 'Special', hideNarrow: true, render: r => r.card.special_state ?? '—' },
    { id: 'stability', label: 'Stability', sortField: 'c.stability', hideNarrow: true, render: r => (r.card.state === 0 ? '—' : formatDays(r.card.stability)) },
    { id: 'difficulty', label: 'Difficulty', sortField: 'c.difficulty', hideNarrow: true, render: r => (r.card.state === 0 ? '—' : r.card.difficulty.toFixed(1)) },
    { id: 'retrievability', label: 'Retrievability', sortField: 'c.retrievability', hideNarrow: true, render: r => (r.extras?.stats.retrievability != null ? `${(r.extras.stats.retrievability * 100).toFixed(0)}%` : '—') },
    { id: 'interval', label: 'Interval', sortField: 'c.interval', render: r => (r.extras?.stats.interval_days != null ? formatDays(r.extras.stats.interval_days) : '—') },
    { id: 'reps', label: 'Reviews', sortField: 'c.reps', hideNarrow: true, render: r => r.extras?.stats.reps ?? '—' },
    { id: 'lapses', label: 'Lapses', sortField: 'c.lapses', hideNarrow: true, render: r => r.extras?.stats.lapses ?? '—' },
    { id: 'leech', label: 'Leech', hideNarrow: true, render: r => (r.extras?.stats.leech ? 'Leech' : '—') },
    { id: 'reviewed', label: 'Last reviewed', sortField: 'c.reviewed', tdStyle: nowrap, render: r => formatDate(r.extras?.stats.last_reviewed) },
    { id: 'created', label: 'Created', sortField: 'c.created_at', tdStyle: nowrap, hideNarrow: true, render: r => formatDateTime(r.card.created_at) },
    { id: 'updated', label: 'Modified', sortField: 'c.updated_at', tdStyle: nowrap, hideNarrow: true, render: r => formatDateTime(r.card.updated_at) },
  ];
}

/** Replaces `card` in `cards` by id, keeping everything else. */
function replaceCard(cards: CardResponse[], card: CardResponse): CardResponse[] {
  return cards.map(c => (c.id === card.id ? card : c));
}

export default function CardsPage() {
  const { credentials } = useAuth();
  const { stateName } = useCardStates();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.max(1, parseInt(searchParams.get('page') ?? '1', 10));
  const pageSizeChoice = useCardsPageSize();
  const pageSize = Number(pageSizeChoice);
  const [cards, setCards] = useState<CardResponse[]>([]);
  // Their notes' text and what their reviews say, by card id, for the pages read from a search
  const [extras, setExtras] = useState<Map<number, CardExtras>>(new Map());
  const { sort, cycle: cycleSort } = useTableSort();
  const shownColumnIds = useCardColumns();
  const allColumns = cardColumns(stateName);
  const columns = allColumns.filter(c => shownColumnIds.includes(c.id));
  // Without a search, the list is read from a search for every card when sorted or showing what
  // only a search returns
  const listBySearch = sort !== null || shownColumnIds.some(c => SEARCH_COLUMNS.includes(c));
  // How many cards there are without a search, which that search says, or which are counted
  const [listTotal, setListTotal] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // The search is kept in the URL so it survives navigating away and back
  const searchQuery = searchParams.get('q');
  const [query, setQuery] = useState(searchQuery ?? '');
  // The page of search results, and how many there are in all
  const [searchResults, setSearchResults] = useState<CardResponse[] | null>(null);
  const [searchTotal, setSearchTotal] = useState(0);
  const [searchError, setSearchError] = useState<SearchInputError | null>(null);
  // The list being stepped through, which is the table unless a card was opened from the leeches
  const [browse, setBrowse] = useState<{ cards: CardResponse[]; index: number } | null>(null);
  // Bumped after a bulk action, to refetch the cards it may have changed
  const [reloadCount, setReloadCount] = useState(0);
  // Whether the next search refetches it after a bulk action, so keeps the card shown
  const reloading = useRef(false);
  const tableBody = useRef<HTMLTableSectionElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const cardPane = useRef<CardDetailHandle>(null);
  const bulkBar = useRef<BulkActionBarHandle>(null);
  const rows = useRowSelection(searchQuery);
  // Whether the search lists its cards in the order the default review shows them
  const queueOrder = searchParams.get('order') === 'queue' && searchQuery !== null;

  const sortField = sort?.field ?? null;
  const sortDescending = sort?.descending ?? false;

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (searchQuery !== null || !listBySearch) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    setLoading(true);
    let cancelled = false;
    searchCardsPage('', page, pageSize, sortField === null ? null : { field: sortField, descending: sortDescending })
      .then(data => {
        if (cancelled) return;
        setCards(data.items.map(m => m.card));
        setExtras(new Map(data.items.map(({ card, ...rest }) => [card.id, rest])));
        setListTotal(data.total);
        setError(null);
      })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, pageSize, searchQuery, reloadCount, listBySearch, sortField, sortDescending]);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (searchQuery !== null || listBySearch) return;
    // Show the page from an earlier visit straight away while it is refetched
    const cached = cachedCardsPage(page, pageSize);
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show the cached page while the page is fetched
    if (cached) setCards(cached);
    setLoading(cached === undefined);
    let cancelled = false;
    // Until it is counted, the last page is the one with fewer cards than a page has
    countCards().then(total => { if (!cancelled) setListTotal(total); }, () => {});
    listCards(page, pageSize)
      .then(data => {
        if (cancelled) return;
        setCards(data);
        setError(null);
        // Prefetch the next page so Next shows it straight away
        if (data.length === pageSize) listCards(page + 1, pageSize).catch(() => {});
      })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, pageSize, searchQuery, reloadCount, listBySearch]);

  const { recent: recentSearches, save: saveRecentSearch, remove: removeRecentSearch } = useRecentSearches('spares_cards_recent_searches');
  // Bumped by each search so searching the same query again refetches it
  const [searchCount, setSearchCount] = useState(0);

  useEffect(() => {
    if (!credentials) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the input follows the URL, e.g. on back
    setQuery(searchQuery ?? '');
    if (searchQuery === null) { setSearchResults(null); return; }
    setLoading(true);
    setError(null);
    let cancelled = false;
    searchCardsPage(searchQuery, page, pageSize, sortField === null ? null : { field: sortField, descending: sortDescending })
      .then(data => {
        if (cancelled) return;
        setSearchResults(data.items.map(m => m.card));
        setExtras(new Map(data.items.map(({ card, ...rest }) => [card.id, rest])));
        setSearchTotal(data.total);
        saveRecentSearch(searchQuery);
        if (!reloading.current) setBrowse(null);
        reloading.current = false;
        setError(null);
        setSearchError(null);
      })
      .catch(e => {
        if (cancelled) return;
        setError(errorMessage(e));
        setSearchError(e instanceof ApiError && e.span ? { query: searchQuery, span: e.span } : null);
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, searchQuery, page, pageSize, searchCount, reloadCount, saveRecentSearch, sortField, sortDescending]);

  function handleSearch(q = query) {
    if (!q.trim()) return;
    setQuery(q);
    setSearchParams(prev => withPage(withSearch(prev, q.trim()), null));
    setSearchCount(n => n + 1);
  }

  const { saved: savedSearches, save: saveSavedSearch, remove: removeSavedSearch, error: savedSearchError } = useSavedSearches();

  function handleSaveSearch() {
    const q = query.trim();
    if (!q) return;
    const name = window.prompt('Save this search as:', savedSearches.find(s => s.query === q)?.name ?? '');
    if (name?.trim()) void saveSavedSearch(name.trim(), q);
  }

  function handleClear() {
    setSearchParams(prev => withPage(withSearch(prev, null), null));
    setBrowse(null);
  }

  const pageCount = Math.max(1, Math.ceil((searchResults !== null ? searchTotal : listTotal ?? 0) / pageSize));
  const hasNextPage = searchResults !== null || listTotal !== null ? page < pageCount : cards.length === pageSize;

  function onCardChanged(card: CardResponse) {
    setCards(prev => replaceCard(prev, card));
    setSearchResults(prev => (prev ? replaceCard(prev, card) : prev));
    setBrowse(prev => (prev ? { ...prev, cards: replaceCard(prev.cards, card) } : prev));
  }

  const detail = useDetailView(browse !== null, () => setBrowse(null));

  const displayedCards = searchResults ?? cards;

  // Where the cards shown are in the default review's queue, looked up when that column is shown
  const showQueue = shownColumnIds.includes('queue');
  const [queueSpots, setQueueSpots] = useState<Map<number, QueueSpot> | null>(null);
  const shownCardIds = displayedCards.map(c => c.id).join(',');
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the old page's places don't apply
    setQueueSpots(null);
    if (!showQueue || shownCardIds === '') return;
    const ids = shownCardIds.split(',');
    let cancelled = false;
    getQueuePositions({ filter: null, match_query: ids.map(id => `c.id=${id}`).join(' or '), limit: ids.length })
      .then(res => {
        if (cancelled) return;
        const spots = new Map<number, QueueSpot>(res.positions.map(p => [p.card_id, p.position]));
        for (const card of res.held_back) spots.set(card.card_id, 'later');
        setQueueSpots(spots);
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [showQueue, shownCardIds, reloadCount]);
  const selected = browse ? browse.cards[browse.index] : null;

  /** Shows what a bulk action, or undoing it, changed. */
  function afterBulkAction() {
    rows.reset();
    // Only a search closes the card shown when it is refetched
    reloading.current = searchQuery !== null;
    setReloadCount(n => n + 1);
    if (!selected) return;
    getCard(selected.id).then(onCardChanged, (e: unknown) => { if (!isNetworkError(e)) setBrowse(null); });
  }

  const header = rows.header(displayedCards);
  const checkedCount = selectedCount(rows.selection, searchTotal);
  function goToPage(n: number) {
    setBrowse(null);
    setSearchParams(prev => withPage(prev, n));
  }
  const prevPage = page > 1 ? () => goToPage(page - 1) : null;
  const nextPage = hasNextPage ? () => goToPage(page + 1) : null;
  /** Lists `size` cards on each page, from the page with the first card of this one. */
  function changePageSize(size: PageSize) {
    setSearchParams(prev => withPage(prev, Math.floor((page - 1) * pageSize / Number(size)) + 1));
    setCardsPageSize(size);
  }
  const keys = useListKeys({
    rows,
    displayed: displayedCards,
    openIndex: selected ? displayedCards.findIndex(c => c.id === selected.id) : -1,
    open: index => setBrowse({ cards: displayedCards, index }),
    close: selected ? () => cardPane.current?.close() : null,
    edit: { description: "Open or close the open card's note", run: selected ? () => cardPane.current?.toggleNote() : null },
    prevPage,
    nextPage,
    searching: searchQuery !== null,
    body: tableBody,
    search: searchInput,
    // As the review page's keys, while the capitals act on the checked cards, which a search
    // checks all of, so that a slip of the Shift key asks first
    extra: [
      { keys: ['s'], codes: ['s'], description: 'Suspend or unsuspend the open card', available: selected !== null, run: () => cardPane.current?.toggleSuspend() },
      { keys: ['b'], codes: ['b'], description: 'Bury or unbury the open card', available: selected !== null, run: () => cardPane.current?.toggleBury() },
      { keys: ['S'], codes: ['S'], description: 'Suspend the checked cards, after asking', available: checkedCount > 0, run: () => bulkBar.current?.choose('Suspend') },
      { keys: ['B'], codes: ['B'], description: 'Bury the checked cards, after asking', available: checkedCount > 0, run: () => bulkBar.current?.choose('Bury') },
    ],
  });

  return (
    <div className="page-wide">
      <style>{`
        .cards-row:hover { background-color: var(--hover); }
        .cards-row-selected { background-color: var(--selected) !important; }
      `}</style>

      <NavbarHelp help={keys} />
      <h2 style={{ marginBottom: 16 }}>Cards</h2>

      <div className="split">
        {/* Hidden rather than unmounted on phones, so it is unchanged when going back to it */}
        <div className="split-list" style={{ flex: 1, minWidth: 0, display: detail.showList ? undefined : 'none' }}>
          <details style={{ marginBottom: 16 }}>
            <summary style={{ cursor: 'pointer', fontSize: 14, marginBottom: 8 }}>Bulk tools (unbury, forget, edit, leeches)</summary>
            <CardTools onOpenCard={(index, list) => setBrowse({ cards: list, index })} />
          </details>

          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: recentSearches.length + savedSearches.length > 0 ? 8 : 16 }}>
            <SearchInput
              ref={searchInput}
              value={query}
              onChange={setQuery}
              onSubmit={handleSearch}
              placeholder="Search cards, e.g. tag=a c.state=review"
              error={searchError}
              style={{ maxWidth: 'none' }}
              inputStyle={{ fontSize: 15, padding: '8px 12px' }}
            />
            <button onClick={() => handleSearch()} className="touch-target-small">Search</button>
            <button onClick={handleSaveSearch} disabled={!query.trim()} className="touch-target-small" title="Save this search, for here and `spares card search --saved`">Save</button>
            {searchResults !== null && <button onClick={handleClear} className="touch-target-small">Clear</button>}
          </div>
          <RecentSearches
            heading="Saved"
            recent={savedSearches}
            label={s => s.name}
            itemTitle={s => s.query}
            onSelect={s => handleSearch(s.query)}
            onRemove={s => { if (window.confirm(`Delete the saved search "${s.name}"?`)) void removeSavedSearch(s); }}
          />
          {savedSearchError && <div style={{ color: 'var(--error)', fontSize: 13, marginBottom: 8, whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>{savedSearchError}</div>}
          <RecentSearches recent={recentSearches} onSelect={handleSearch} onRemove={removeRecentSearch} style={{ marginBottom: 16 }} />

          <div style={{ display: 'flex', gap: 12, alignItems: 'center', flexWrap: 'wrap', fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>
            {searchResults !== null && <span>{searchTotal} result{searchTotal !== 1 ? 's' : ''} for "{searchQuery}"</span>}
            {searchResults !== null && (
              <label style={{ display: 'flex', gap: 4, alignItems: 'center' }}>
                Order
                <select
                  value={queueOrder ? 'queue' : 'search'}
                  onChange={e => setSearchParams(prev => {
                    const next = new URLSearchParams(prev);
                    // The queue order has no columns to sort by
                    if (e.target.value === 'queue') { next.set('order', 'queue'); next.delete('sort'); next.delete('dir'); }
                    else next.delete('order');
                    return next;
                  })}
                  title="Queue position lists the cards in the order the default review shows them today, then why it would not show the others"
                >
                  <option value="search">Search</option>
                  <option value="queue">Queue position</option>
                </select>
              </label>
            )}
            {!queueOrder && (
              <span style={{ marginLeft: 'auto' }}>
                <ColumnPicker columns={allColumns} shown={shownColumnIds} onChange={setCardColumns} />
              </span>
            )}
          </div>

          <BulkActionBar
            ref={bulkBar}
            noun="card"
            selection={rows.selection}
            query={searchQuery}
            total={searchTotal}
            onSelectAll={() => rows.selectAll(displayedCards)}
            onSelectNone={rows.selectNone}
            onInvert={() => rows.invert(displayedCards)}
            search={async q => (await searchCards(q)).map(c => c.id)}
            actions={BULK_ACTIONS}
            onDone={afterBulkAction}
          />

          {error && <pre style={{ color: 'var(--error)', marginBottom: 12, marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>}
          {loading && <div>Loading…</div>}

          {queueOrder && searchQuery !== null && (
            <QueueOrder
              query={searchQuery}
              onOpen={c => { getCard(c.card_id).then(card => setBrowse({ cards: [card], index: 0 }), (e: unknown) => setError(errorMessage(e))); }}
            />
          )}

          {!loading && !queueOrder && (
            <div className="table-scroll">
              <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
                <thead>
                  <tr>
                    <th style={{ ...th, width: 1, paddingRight: 0 }}>
                      <Checkbox
                        checked={header.checked}
                        indeterminate={header.indeterminate}
                        onClick={header.toggle}
                        label={searchQuery !== null ? 'Select all cards found' : 'Select the cards on this page'}
                      />
                    </th>
                    {columns.map(column => <ColumnHeader key={column.id} column={column} sort={sort} onSort={cycleSort} />)}
                  </tr>
                </thead>
                <tbody ref={tableBody}>
                  {displayedCards.map((card, i) => (
                    <tr
                      key={card.id}
                      className={`cards-row${selected?.id === card.id ? ' cards-row-selected' : ''}`}
                      onClick={() => setBrowse(selected?.id === card.id ? null : { cards: displayedCards, index: i })}
                      style={{ cursor: 'pointer' }}
                    >
                      <td style={checkTd} onClick={e => { e.stopPropagation(); rows.toggleRow(displayedCards, i, e.shiftKey); }}>
                        <Checkbox checked={rows.isSelected(card.id)} onClick={e => rows.toggleRow(displayedCards, i, e.shiftKey)} label={`Select card ${card.id}`} />
                      </td>
                      {columns.map(column => {
                        const className = [column.hideNarrow && 'hide-narrow', column.id === 'text' && 'clamp-narrow'].filter(Boolean).join(' ') || undefined;
                        return (
                          <td key={column.id} style={column.tdStyle ? { ...td, ...column.tdStyle } : td} className={className}>
                            {column.render({ card, extras: extras.get(card.id), queue: queueSpots ? queueSpots.get(card.id) ?? null : undefined })}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                  {displayedCards.length === 0 && (
                    <tr>
                      <td colSpan={columns.length + 1} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>
                        {searchResults === null && page === 1
                          ? <>No cards yet. Cards are made by the clozes of notes, written on the <Link to="/notes">Notes</Link> page.</>
                          : 'No cards found'}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}

          {!queueOrder && <Pagination page={page} pageCount={searchResults !== null || listTotal !== null ? pageCount : null} prevPage={prevPage} nextPage={nextPage} goToPage={goToPage} pageSize={pageSizeChoice} setPageSize={changePageSize} />}
        </div>

        <div className="split-detail" style={{ flex: 1, minWidth: 0 }}>
          {!detail.showList && <button onClick={detail.back} className="touch-target" style={{ marginBottom: 12 }}>← Back to cards</button>}
          {browse && selected
            ? <CardDetail
                ref={cardPane}
                key={selected.id}
                card={selected}
                index={browse.index}
                total={browse.cards.length}
                onGoTo={index => setBrowse({ ...browse, index })}
                onClose={detail.back}
                onCardChanged={onCardChanged}
              />
            : !detail.isNarrow && <div style={{ color: 'var(--text-faint)', fontSize: 14, paddingTop: 8 }}>Select a card to view it.</div>
          }
        </div>
      </div>
    </div>
  );
}
