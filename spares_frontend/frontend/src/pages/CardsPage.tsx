import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { ApiError, cachedCardsPage, forgetCard, getCard, isNetworkError, listCards, listManualTagNames, modifyNoteTags, searchCards, searchCardsPage, updateCards } from '../api/client';
import BulkActionBar, { Checkbox, type BulkAction } from '../components/BulkActionBar';
import { FILTERED_TAG_ACTION } from '../components/bulkActions';
import CardDetail from '../components/CardDetail';
import CardTools from '../components/CardTools';
import Navbar from '../components/Navbar';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import { useAuth } from '../hooks/useAuth';
import { useDetailView } from '../hooks/useDetailView';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useRowSelection } from '../hooks/useRowSelection';
import { useSelectionKeys } from '../hooks/useSelectionKeys';
import { useSavedSearches } from '../hooks/useSavedSearches';
import type { IdsOrQuery } from '../selection';
import { STATE_LABELS, type CardResponse, type UpdateCardsRequest } from '../types/spares';
import { errorMessage, formatNoteId, tagList, td, th, withPage, withSearch } from '../utils';

const PAGE_SIZE = 20;
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

/** Replaces `card` in `cards` by id, keeping everything else. */
function replaceCard(cards: CardResponse[], card: CardResponse): CardResponse[] {
  return cards.map(c => (c.id === card.id ? card : c));
}

export default function CardsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.max(1, parseInt(searchParams.get('page') ?? '1', 10));
  const [cards, setCards] = useState<CardResponse[]>([]);
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
  const rows = useRowSelection(searchQuery);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (searchQuery !== null) return;
    // Show the page from an earlier visit straight away while it is refetched
    const cached = cachedCardsPage(page, PAGE_SIZE);
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    if (cached) setCards(cached);
    setLoading(cached === undefined);
    let cancelled = false;
    listCards(page, PAGE_SIZE)
      .then(data => {
        if (cancelled) return;
        setCards(data);
        setError(null);
        // Prefetch the next page so Next shows it straight away
        if (data.length === PAGE_SIZE) listCards(page + 1, PAGE_SIZE).catch(() => {});
      })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, searchQuery, reloadCount]);

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
    searchCardsPage(searchQuery, page, PAGE_SIZE)
      .then(data => {
        if (cancelled) return;
        setSearchResults(data.items);
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
  }, [credentials, searchQuery, page, searchCount, reloadCount, saveRecentSearch]);

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

  const pageCount = Math.max(1, Math.ceil(searchTotal / PAGE_SIZE));
  const hasNextPage = searchResults !== null ? page < pageCount : cards.length === PAGE_SIZE;

  function onCardChanged(card: CardResponse) {
    setCards(prev => replaceCard(prev, card));
    setSearchResults(prev => (prev ? replaceCard(prev, card) : prev));
    setBrowse(prev => (prev ? { ...prev, cards: replaceCard(prev.cards, card) } : prev));
  }

  const detail = useDetailView(browse !== null, () => setBrowse(null));

  const displayedCards = searchResults ?? cards;
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
  const keys = useSelectionKeys({ rows, displayed: displayedCards, openIndex: selected ? displayedCards.findIndex(c => c.id === selected.id) : -1, searching: searchQuery !== null });

  return (
    <div className="page-wide">
      <style>{`
        .cards-row:hover { background-color: var(--hover); }
        .cards-row-selected { background-color: var(--selected) !important; }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} help={keys} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Cards</h2>

      <div className="split">
        {/* Hidden rather than unmounted on phones, so it is unchanged when going back to it */}
        <div style={{ flex: 1, minWidth: 0, display: detail.showList ? undefined : 'none' }}>
          <details style={{ marginBottom: 16 }}>
            <summary style={{ cursor: 'pointer', fontSize: 14, marginBottom: 8 }}>Bulk tools (unbury, forget, edit, leeches)</summary>
            <CardTools onOpenCard={(index, list) => setBrowse({ cards: list, index })} />
          </details>

          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: recentSearches.length + savedSearches.length > 0 ? 8 : 16 }}>
            <SearchInput
              value={query}
              onChange={setQuery}
              onSubmit={handleSearch}
              placeholder="Search cards, e.g. tag=a c.state=review"
              error={searchError}
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

          {searchResults !== null && (
            <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>
              {searchTotal} result{searchTotal !== 1 ? 's' : ''} for "{searchQuery}"
            </div>
          )}

          <BulkActionBar
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

          {!loading && (
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
                    <th style={th}>ID</th>
                    <th style={th}>Note</th>
                    <th style={th} className="hide-narrow">Order</th>
                    <th style={th}>State</th>
                    <th style={th}>Due</th>
                    <th style={th} className="hide-narrow">Retention</th>
                    <th style={th} className="hide-narrow">Special</th>
                  </tr>
                </thead>
                <tbody>
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
                      <td style={td}>{card.id}</td>
                      <td style={td}>{formatNoteId(card.note_id)}</td>
                      <td style={td} className="hide-narrow">{card.order}</td>
                      <td style={td}>{STATE_LABELS[card.state] ?? card.state}</td>
                      <td style={{ ...td, whiteSpace: 'nowrap' }}>{new Date(card.due).toLocaleDateString()}</td>
                      <td style={td} className="hide-narrow">{card.desired_retention}</td>
                      <td style={td} className="hide-narrow">{card.special_state ?? '—'}</td>
                    </tr>
                  ))}
                  {displayedCards.length === 0 && (
                    <tr>
                      <td colSpan={8} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>
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

          <div style={{ marginTop: 16, display: 'flex', gap: 8, alignItems: 'center' }}>
            <button disabled={page <= 1} className="touch-target-small" onClick={() => { setBrowse(null); setSearchParams(prev => withPage(prev, page - 1)); }}>Prev</button>
            <span style={{ fontSize: 13 }}>Page {page}{searchResults !== null && ` of ${pageCount}`}</span>
            <button disabled={!hasNextPage} className="touch-target-small" onClick={() => { setBrowse(null); setSearchParams(prev => withPage(prev, page + 1)); }}>Next</button>
          </div>
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          {!detail.showList && <button onClick={detail.back} className="touch-target" style={{ marginBottom: 12 }}>← Back to cards</button>}
          {browse && selected
            ? <CardDetail
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
