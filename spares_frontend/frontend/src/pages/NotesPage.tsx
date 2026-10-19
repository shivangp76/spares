import { useEffect, useRef, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { ApiError, cachedNotesPage, changeNotesParser, deleteNotes, exportNotes, getNote, isNetworkError, listManualTagNames, listNotes, listParsers, modifyNoteTags, searchNotes, searchNotesPage } from '../api/client';
import { useAuth } from '../hooks/useAuth';
import { useDetailView } from '../hooks/useDetailView';
import { useRecentSearches } from '../hooks/useRecentSearches';
import { useRowSelection } from '../hooks/useRowSelection';
import { useSelectionKeys } from '../hooks/useSelectionKeys';
import { useSavedSearches } from '../hooks/useSavedSearches';
import BulkActionBar, { Checkbox, type BulkAction } from '../components/BulkActionBar';
import { FILTERED_TAG_ACTION } from '../components/bulkActions';
import Navbar from '../components/Navbar';
import RecentSearches from '../components/RecentSearches';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import type { IdsOrQuery, RowSelection } from '../selection';
import NewNoteForm from '../components/NewNoteForm';
import NoteDetail from '../components/NoteDetail';
import type { NoteResponse } from '../types/spares';
import { SYNC_CONFLICT_TAG, downloadText, errorMessage, formatNoteId, highlightPreview, keepShown, tagList, td, th, withPage, withSearch } from '../utils';

const PAGE_SIZE = 20;
const DATA_PREVIEW_LEN = 100;
const dataTd: React.CSSProperties = { ...td, maxWidth: 300, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };
const checkTd: React.CSSProperties = { ...td, width: 1, paddingRight: 0 };

/** A query that finds the notes `selector` picks, for an endpoint that only takes a query. */
function queryOf(selector: IdsOrQuery): string {
  return 'Query' in selector ? selector.Query : selector.Ids.map(id => `id=${formatNoteId(id)}`).join(' or ');
}

/** The tags of `notes`, most used first, leaving out filtered tags, which can't be added or removed by hand. */
async function tagsByUse(notes: NoteResponse[]): Promise<string[]> {
  const manual = new Set(await listManualTagNames());
  const uses = new Map<string, number>();
  for (const tag of notes.flatMap(n => n.tags)) {
    if (manual.has(tag)) uses.set(tag, (uses.get(tag) ?? 0) + 1);
  }
  return [...uses].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([tag]) => tag);
}

export default function NotesPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.max(1, parseInt(searchParams.get('page') ?? '1', 10));
  const [notes, setNotes] = useState<NoteResponse[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // The search is kept in the URL so it survives navigating away and back
  const searchQuery = searchParams.get('q');
  const [query, setQuery] = useState(searchQuery ?? '');
  // The page of search results, how many there are in all, and where each note matches
  const [searchResults, setSearchResults] = useState<NoteResponse[] | null>(null);
  const [searchTotal, setSearchTotal] = useState(0);
  const [searchMatches, setSearchMatches] = useState<Map<number, [number, number][]>>(new Map());
  const [searchError, setSearchError] = useState<SearchInputError | null>(null);
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);
  const [creating, setCreating] = useState(false);
  // The new note form stays mounted, hidden, while other notes are viewed, so its unsaved
  // contents are still there when New note is clicked again
  const [hasDraft, setHasDraft] = useState(false);
  const [paneStatus, setPaneStatus] = useState<string | null>(null);
  // Bumped after a bulk action, to refetch the notes it may have changed
  const [reloadCount, setReloadCount] = useState(0);
  // Bumped when the shown note is changed by a bulk action, so its editor starts again from it
  const [detailVersion, setDetailVersion] = useState(0);
  // The notes seen on any page, to find the tags of those checked
  const seenNotes = useRef(new Map<number, NoteResponse>());
  const rows = useRowSelection(searchQuery);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    if (searchQuery !== null) return;
    // Show the page from an earlier visit straight away while it is refetched
    const cached = cachedNotesPage(page, PAGE_SIZE);
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    if (cached) setNotes(cached);
    setLoading(cached === undefined);
    let cancelled = false;
    listNotes(page, PAGE_SIZE)
      .then(data => {
        if (cancelled) return;
        setNotes(data);
        for (const note of data) seenNotes.current.set(note.id, note);
        setError(null);
        // Prefetch the next page so Next shows it straight away
        if (data.length === PAGE_SIZE) listNotes(page + 1, PAGE_SIZE).catch(() => {});
      })
      .catch(e => { if (!cancelled) setError(errorMessage(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [credentials, navigate, page, searchQuery, reloadCount]);

  const { recent: recentSearches, save: saveRecentSearch, remove: removeRecentSearch } = useRecentSearches('spares_notes_recent_searches');
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
    searchNotesPage(searchQuery, page, PAGE_SIZE)
      .then(data => {
        if (cancelled) return;
        setSearchResults(data.items.map(m => m.note));
        for (const { note } of data.items) seenNotes.current.set(note.id, note);
        setSearchTotal(data.total);
        setSearchMatches(new Map(data.items.map(m => [m.note.id, m.matches])));
        saveRecentSearch(searchQuery);
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
    setSearchParams(prev => withPage(withSearch(prev, q), null));
    setSearchCount(n => n + 1);
  }

  const { saved: savedSearches, save: saveSavedSearch, remove: removeSavedSearch, error: savedSearchError } = useSavedSearches();

  function handleSaveSearch() {
    const q = query.trim();
    if (!q) return;
    const name = window.prompt('Save this search as:', savedSearches.find(s => s.query === q)?.name ?? '');
    if (name?.trim()) void saveSavedSearch(name.trim(), q);
  }

  function startNewNote() {
    setSelectedNote(null);
    setPaneStatus(null);
    setCreating(true);
    setHasDraft(true);
  }

  function handleClear() {
    setSearchParams(prev => withPage(withSearch(prev, null), null));
  }

  const pageCount = Math.max(1, Math.ceil(searchTotal / PAGE_SIZE));
  const hasNextPage = searchResults !== null ? page < pageCount : notes.length === PAGE_SIZE;

  const displayedNotes = searchResults ?? notes;
  // -1 if the selected note was opened from a link and isn't in the table
  const selectedIndex = selectedNote ? displayedNotes.findIndex(n => n.id === selectedNote.id) : -1;

  function selectNote(note: NoteResponse | null) {
    setSelectedNote(note);
    setCreating(false);
    setPaneStatus(null);
  }

  const detail = useDetailView(selectedNote !== null || creating, () => selectNote(null));

  async function openLinkedNote(noteId: number) {
    try {
      selectNote(await getNote(noteId));
    } catch (e) {
      setPaneStatus(errorMessage(e));
    }
  }

  async function deleteSelected() {
    if (!selectedNote) return;
    const id = selectedNote.id;
    if (!window.confirm(`Delete note ${id} and its cards?`)) return;
    try {
      await deleteNotes({ Ids: [id] });
      setNotes(prev => prev.filter(n => n.id !== id));
      if (searchResults?.some(n => n.id === id)) setSearchTotal(total => total - 1);
      setSearchResults(prev => (prev ? prev.filter(n => n.id !== id) : prev));
      setSelectedNote(null);
      setPaneStatus(`Note ${id} deleted.`);
    } catch (e) {
      setPaneStatus(errorMessage(e));
    }
  }

  /** The notes `selection` has, for the tags they have. */
  async function selectedNotes(selection: RowSelection): Promise<NoteResponse[]> {
    if (!selection.all) return [...selection.ids].flatMap(id => seenNotes.current.get(id) ?? []);
    if (searchQuery === null) return [];
    return (await searchNotes(searchQuery)).filter(n => !selection.ids.has(n.id));
  }

  async function tagNotes(selector: IdsOrQuery, add: string[], remove: string[]) {
    const { notes: changed, eventId } = await modifyNoteTags(selector, add, remove);
    const message = add.length ? `Added ${tagList(add)} to ${changed.length} note(s).` : `Removed ${tagList(remove)} from ${changed.length} note(s).`;
    return { message, eventIds: eventId === null ? [] : [eventId] };
  }

  const bulkActions: BulkAction[] = [
    {
      label: 'Add tags',
      input: { kind: 'tags', suggestions: listManualTagNames },
      confirm: (count, tags) => `Add ${tagList(tags)} to ${count} note(s)?`,
      run: (selector, tags) => tagNotes(selector, tags, []),
    },
    {
      label: 'Remove tags',
      input: { kind: 'tags', suggestions: async () => tagsByUse(await selectedNotes(rows.selection)) },
      confirm: (count, tags) => `Remove ${tagList(tags)} from ${count} note(s)?`,
      run: (selector, tags) => tagNotes(selector, [], tags),
    },
    {
      label: 'Change parser',
      input: { kind: 'choice', options: async () => (await listParsers(1, 1_000)).map(p => ({ value: p.name, label: p.name })) },
      confirm: (count, [name]) => `Change the parser of ${count} note(s) to ${name}? Their text must be valid for it, and cards it no longer makes are deleted.`,
      run: async (selector, [name]) => {
        const parser = (await listParsers(1, 1_000)).find(p => p.name === name);
        if (!parser) throw new Error(`No parser named ${name}`);
        const { notes: changed, eventId } = await changeNotesParser(selector, parser.id);
        return { message: `Changed the parser of ${changed.length} note(s) to ${name}.`, eventIds: eventId === null ? [] : [eventId] };
      },
    },
    {
      label: 'Export',
      run: async (selector, _, count) => {
        const files = Object.entries(await exportNotes(queryOf(selector)));
        for (const [extension, text] of files) downloadText(`export.${extension}`, text);
        return { message: `Exported ${count} note(s) to ${files.map(([extension]) => `export.${extension}`).join(', ') || 'no files'}.`, eventIds: [] };
      },
    },
    FILTERED_TAG_ACTION,
    {
      label: 'Delete',
      danger: true,
      confirm: count => `Delete ${count} note(s) and their cards?`,
      run: async (selector, _, count) => {
        await deleteNotes(selector);
        // The server doesn't return the event it records, so no Undo is offered
        return { message: `Deleted ${count} note(s).`, eventIds: [] };
      },
    },
  ];

  /** Shows what a bulk action, or undoing it, changed. */
  function afterBulkAction() {
    rows.reset();
    setReloadCount(n => n + 1);
    if (!selectedNote) return;
    getNote(selectedNote.id).then(
      note => { setSelectedNote(note); setDetailVersion(v => v + 1); },
      (e: unknown) => { if (!isNetworkError(e)) setSelectedNote(null); },
    );
  }

  const header = rows.header(displayedNotes);
  const keys = useSelectionKeys({ rows, displayed: displayedNotes, openIndex: selectedIndex, searching: searchQuery !== null });

  return (
    <div className="page-wide">
      <style>{`
        .notes-row:hover { background-color: var(--hover); }
        .notes-row-selected { background-color: var(--selected) !important; }
      `}</style>

      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} help={keys} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Notes</h2>

      <div className="split">
        {/* Hidden rather than unmounted on phones, so it is unchanged when going back to it */}
        <div style={{ flex: 1, minWidth: 0, display: detail.showList ? undefined : 'none' }}>
          <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: recentSearches.length + savedSearches.length > 0 ? 8 : 16 }}>
            <SearchInput
              value={query}
              onChange={setQuery}
              onSubmit={handleSearch}
              placeholder="Search notes…"
              error={searchError}
            />
            <button onClick={() => handleSearch()} className="touch-target-small">Search</button>
            <button onClick={handleSaveSearch} disabled={!query.trim()} className="touch-target-small" title="Save this search, for here and `spares note search --saved`">Save</button>
            {searchResults !== null && <button onClick={handleClear} className="touch-target-small">Clear</button>}
            <button onClick={startNewNote} className="touch-target-small" style={{ marginLeft: 'auto' }}>New note</button>
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
            noun="note"
            selection={rows.selection}
            query={searchQuery}
            total={searchTotal}
            onSelectAll={() => rows.selectAll(displayedNotes)}
            onSelectNone={rows.selectNone}
            onInvert={() => rows.invert(displayedNotes)}
            search={async q => (await searchNotes(q)).map(n => n.id)}
            actions={bulkActions}
            onDone={afterBulkAction}
          />

          {error && <pre style={{ color: 'var(--error)', marginBottom: 12, marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>}
          {loading && <div>Loading…</div>}

          {!loading && (
            <div className="table-scroll">
              <table style={{ width: '100%', borderCollapse: 'collapse' }}>
                <thead>
                  <tr>
                    <th style={{ ...th, width: 1, paddingRight: 0 }}>
                      <Checkbox
                        checked={header.checked}
                        indeterminate={header.indeterminate}
                        onClick={header.toggle}
                        label={searchQuery !== null ? 'Select all notes found' : 'Select the notes on this page'}
                      />
                    </th>
                    <th style={th}>ID</th>
                    <th style={th}>Data</th>
                    <th style={th}>Tags</th>
                    <th style={th} className="hide-narrow">Keywords</th>
                    <th style={th} className="hide-narrow">Cards</th>
                  </tr>
                </thead>
                <tbody>
                  {displayedNotes.map((note, i) => (
                    <tr
                      key={note.id}
                      className={`notes-row${selectedNote?.id === note.id ? ' notes-row-selected' : ''}`}
                      onClick={() => selectNote(selectedNote?.id === note.id ? null : note)}
                      style={{ cursor: 'pointer' }}
                    >
                      <td style={checkTd} onClick={e => { e.stopPropagation(); rows.toggleRow(displayedNotes, i, e.shiftKey); }}>
                        <Checkbox checked={rows.isSelected(note.id)} onClick={e => rows.toggleRow(displayedNotes, i, e.shiftKey)} label={`Select note ${note.id}`} />
                      </td>
                      <td style={td}>
                        {formatNoteId(note.id)}
                        {note.tags.includes(SYNC_CONFLICT_TAG) && <span className="sync-conflict-badge" title="A sync could not merge this note. Resolve it with `spares sync conflicts`." style={{ marginLeft: 6 }}>conflict</span>}
                      </td>
                      <td style={dataTd} className="clamp-narrow" title={note.data}>
                        <span>
                          {highlightPreview(note.data, searchMatches.get(note.id) ?? [], DATA_PREVIEW_LEN).map((part, i) =>
                            part.mark ? <mark key={i} className="search-match">{part.text}</mark> : part.text,
                          )}
                        </span>
                      </td>
                      <td style={td}>{note.tags.join(', ') || '—'}</td>
                      <td style={td} className="hide-narrow">
                        {note.keywords.slice(0, 4).join(', ')}
                        {note.keywords.length > 4 ? '…' : ''}
                      </td>
                      <td style={td} className="hide-narrow">{note.card_count}</td>
                    </tr>
                  ))}
                  {displayedNotes.length === 0 && (
                    <tr>
                      <td colSpan={6} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>
                        {searchResults === null && page === 1 ? (
                          <>No notes yet. <button onClick={startNewNote} style={{ padding: '2px 8px', fontSize: 12 }}>Write one</button>, or press <kbd>?</kbd> to learn how.</>
                        ) : 'No notes found'}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}

          <div style={{ marginTop: 16, display: 'flex', gap: 8, alignItems: 'center' }}>
            <button disabled={page <= 1} className="touch-target-small" onClick={() => setSearchParams(prev => withPage(prev, page - 1))}>Prev</button>
            <span style={{ fontSize: 13 }}>Page {page}{searchResults !== null && ` of ${pageCount}`}</span>
            <button disabled={!hasNextPage} className="touch-target-small" onClick={() => setSearchParams(prev => withPage(prev, page + 1))}>Next</button>
          </div>
        </div>

        <div style={{ flex: 1, minWidth: 0 }}>
          {!detail.showList && <button onClick={detail.back} className="touch-target" style={{ marginBottom: 12 }}>← Back to notes</button>}
          {paneStatus && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 8 }}>{paneStatus}</div>}
          {hasDraft && (
            <div style={{ display: creating ? undefined : 'none' }}>
              <NewNoteForm
                onCreated={note => {
                  setHasDraft(false);
                  selectNote(note);
                  setPaneStatus(`Note ${formatNoteId(note.id)} created.`);
                }}
                onCancel={() => { setHasDraft(false); detail.back(); }}
              />
            </div>
          )}
          {creating ? null : selectedNote ? (
            <>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 8, fontSize: 13 }}>
                {selectedIndex >= 0 && (
                  <>
                    {/* Wraps around at either end, as in the CLI */}
                    <button onClick={() => selectNote(displayedNotes[(selectedIndex + displayedNotes.length - 1) % displayedNotes.length])} disabled={displayedNotes.length < 2} className="touch-target-small">Previous</button>
                    <span>Note {selectedIndex + 1} of {displayedNotes.length}</span>
                    <button onClick={() => selectNote(displayedNotes[(selectedIndex + 1) % displayedNotes.length])} disabled={displayedNotes.length < 2} className="touch-target-small">Next</button>
                  </>
                )}
                <button onClick={deleteSelected} className="touch-target-small" style={{ marginLeft: 'auto', color: 'var(--danger)' }}>Delete note</button>
              </div>
              <NoteDetail
                key={`${selectedNote.id}:${detailVersion}`}
                note={selectedNote}
                onClose={detail.back}
                onNoteUpdated={(updated) => {
                  setSelectedNote(prev => keepShown(prev, updated));
                  setNotes(prev => prev.map(n => n.id === updated.id ? updated : n));
                  setSearchResults(prev => prev ? prev.map(n => n.id === updated.id ? updated : n) : prev);
                }}
                onOpenNote={openLinkedNote}
              />
            </>
          ) : !detail.isNarrow && (
            <div style={{ color: 'var(--text-faint)', fontSize: 14, paddingTop: 8 }}>Select a note to see details.</div>
          )}
        </div>
      </div>
    </div>
  );
}
