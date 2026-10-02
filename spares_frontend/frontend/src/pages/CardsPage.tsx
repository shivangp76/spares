import { useEffect, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { cachedCardsPage, listCards, searchCards } from '../api/client';
import CardDetail from '../components/CardDetail';
import CardTools from '../components/CardTools';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { STATE_LABELS, type CardResponse } from '../types/spares';
import { td, th, withSearch } from '../utils';

const PAGE_SIZE = 20;

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
  const [searchResults, setSearchResults] = useState<CardResponse[] | null>(null);
  // The list being stepped through, which is the table unless a card was opened from the leeches
  const [browse, setBrowse] = useState<{ cards: CardResponse[]; index: number } | null>(null);

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
      .catch(e => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, searchQuery]);

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
    searchCards(searchQuery)
      .then(data => { if (!cancelled) { setSearchResults(data); setBrowse(null); setError(null); } })
      .catch(e => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, searchQuery, searchCount]);

  function handleSearch() {
    if (!query.trim()) return;
    setSearchParams(prev => withSearch(prev, query.trim()));
    setSearchCount(n => n + 1);
  }

  function handleClear() {
    setSearchParams(prev => withSearch(prev, null));
    setBrowse(null);
  }

  function onCardChanged(card: CardResponse) {
    setCards(prev => replaceCard(prev, card));
    setSearchResults(prev => (prev ? replaceCard(prev, card) : prev));
    setBrowse(prev => (prev ? { ...prev, cards: replaceCard(prev.cards, card) } : prev));
  }

  const displayedCards = searchResults ?? cards;
  const selected = browse ? browse.cards[browse.index] : null;

  return (
    <div style={{ padding: 24 }}>
      <style>{`
        .cards-split { display: flex; flex-direction: row; gap: 24px; align-items: flex-start; }
        @media (max-width: 768px) { .cards-split { flex-direction: column; } }
        .cards-row:hover { background-color: #f5f5f5; }
        .cards-row-selected { background-color: #f0f4ff !important; }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Cards</h2>

      <div className="cards-split">
        <div style={{ flex: 1, minWidth: 0 }}>
          <details style={{ marginBottom: 16 }}>
            <summary style={{ cursor: 'pointer', fontSize: 14, marginBottom: 8 }}>Bulk tools (unbury, forget, edit, leeches)</summary>
            <CardTools onOpenCard={(index, list) => setBrowse({ cards: list, index })} />
          </details>

          <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
            <input
              type="text"
              value={query}
              onChange={e => setQuery(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleSearch()}
              placeholder="Search cards, e.g. tag=a c.state=2"
              style={{ padding: '6px 10px', fontSize: 14, flex: 1, maxWidth: 400 }}
            />
            <button onClick={handleSearch}>Search</button>
            {searchResults !== null && <button onClick={handleClear}>Clear</button>}
          </div>

          {searchResults !== null && (
            <div style={{ fontSize: 13, color: '#555', marginBottom: 8 }}>
              {searchResults.length} result{searchResults.length !== 1 ? 's' : ''} for "{searchQuery}"
            </div>
          )}

          {error && <div style={{ color: 'red', marginBottom: 12 }}>Error: {error}</div>}
          {loading && <div>Loading…</div>}

          {!loading && (
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
              <thead>
                <tr>
                  <th style={th}>ID</th>
                  <th style={th}>Note</th>
                  <th style={th}>Order</th>
                  <th style={th}>State</th>
                  <th style={th}>Due</th>
                  <th style={th}>Retention</th>
                  <th style={th}>Special</th>
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
                    <td style={td}>{card.id}</td>
                    <td style={td}>{card.note_id}</td>
                    <td style={td}>{card.order}</td>
                    <td style={td}>{STATE_LABELS[card.state] ?? card.state}</td>
                    <td style={{ ...td, whiteSpace: 'nowrap' }}>{new Date(card.due).toLocaleDateString()}</td>
                    <td style={td}>{card.desired_retention}</td>
                    <td style={td}>{card.special_state ?? '—'}</td>
                  </tr>
                ))}
                {displayedCards.length === 0 && (
                  <tr><td colSpan={7} style={{ ...td, color: '#888', textAlign: 'center' }}>No cards found</td></tr>
                )}
              </tbody>
            </table>
          )}

          {searchResults === null && (
            <div style={{ marginTop: 16, display: 'flex', gap: 8, alignItems: 'center' }}>
              <button disabled={page <= 1} onClick={() => { setBrowse(null); setSearchParams({ page: String(page - 1) }); }}>Prev</button>
              <span style={{ fontSize: 13 }}>Page {page}</span>
              <button disabled={cards.length < PAGE_SIZE} onClick={() => { setBrowse(null); setSearchParams({ page: String(page + 1) }); }}>Next</button>
            </div>
          )}
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          {browse && selected
            ? <CardDetail
                key={selected.id}
                card={selected}
                index={browse.index}
                total={browse.cards.length}
                onGoTo={index => setBrowse({ ...browse, index })}
                onClose={() => setBrowse(null)}
                onCardChanged={onCardChanged}
              />
            : <div style={{ color: '#999', fontSize: 14, paddingTop: 8 }}>Select a card to view it.</div>
          }
        </div>
      </div>
    </div>
  );
}
