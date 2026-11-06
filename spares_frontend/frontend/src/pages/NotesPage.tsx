import { useEffect, useRef, useState } from "react";
import { useDataVersion } from "../dataVersion";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import {
  ApiError,
  cachedNotesPage,
  changeNotesParser,
  countNotes,
  deleteNotes,
  exportNotes,
  getNote,
  shareBundle,
  shareSummary as fetchShareSummary,
  isNetworkError,
  listManualTagNames,
  listNotes,
  listParsers,
  modifyNoteTags,
  searchNotes,
  searchNotesPage,
} from "../api/client";
import { useAuth } from "../hooks/useAuth";
import { useDetailView } from "../hooks/useDetailView";
import { useRecentSearches } from "../hooks/useRecentSearches";
import { useRowSelection } from "../hooks/useRowSelection";
import { useListKeys } from "../hooks/useListKeys";
import { useSavedSearches } from "../hooks/useSavedSearches";
import ActionResult, { type ActionOutcome } from "../components/ActionResult";
import BulkActionBar, {
  Checkbox,
  type BulkAction,
} from "../components/BulkActionBar";
import { FILTERED_TAG_ACTION } from "../components/bulkActions";
import ImportDialog from "../components/ImportDialog";
import { NavbarHelp } from "../components/Navbar";
import RecentSearches from "../components/RecentSearches";
import SavedSearches from "../components/SavedSearches";
import SearchInput, { type SearchInputError } from "../components/SearchInput";
import type { IdsOrQuery, RowSelection } from "../selection";
import NewNoteForm from "../components/NewNoteForm";
import NoteDetail, { type NotePaneHandle } from "../components/NoteDetail";
import Pagination from "../components/Pagination";
import {
  setNoteColumns,
  setNotesPageSize,
  useNoteColumns,
  useNotesPageSize,
  type NoteColumn,
  type PageSize,
} from "../preferences";
import {
  ColumnHeader,
  ColumnPicker,
  type Column,
} from "../components/tableColumns";
import { useTableSort } from "../hooks/useTableSort";
import { useTableWidths } from "../hooks/useTableWidths";
import type { NoteResponse, ShareSummary } from "../types/spares";
import {
  SYNC_CONFLICT_TAG,
  downloadBlob,
  downloadText,
  errorMessage,
  formatDate,
  formatDateTime,
  formatNoteId,
  highlightPreview,
  keepShown,
  tagList,
  td,
  th,
  withPage,
  withSearch,
} from "../utils";
import PageTitle from "../components/PageTitle";

const DATA_PREVIEW_LEN = 100;
const dataTd: React.CSSProperties = {
  ...td,
  maxWidth: 300,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};
const checkTd: React.CSSProperties = { ...td, width: 1, paddingRight: 0 };

/** What an export to share holds, from its summary, of `count` notes selected. */
function shareSummary(summary: ShareSummary, count: number): string {
  const shared = count - summary.private_notes_left_out;
  const lines = [
    `${shared} note(s), with ${summary.media.length} image, audio and image occlusion file(s).`,
  ];
  if (summary.private_notes_left_out > 0)
    lines.push(
      `${summary.private_notes_left_out} private note(s) are left out.`,
    );
  if (summary.missing_media.length > 0) {
    lines.push(
      `These paths have no file, so the notes still name them. Check that they name nothing private:\n${summary.missing_media.join("\n")}`,
    );
  }
  return lines.join("\n");
}

/** A query that finds the notes `selector` picks, for an endpoint that only takes a query. */
function queryOf(selector: IdsOrQuery): string {
  return "Query" in selector
    ? selector.Query
    : selector.Ids.map((id) => `id=${formatNoteId(id)}`).join(" or ");
}

/** The tags of `notes`, most used first, leaving out filtered tags, which can't be added or removed by hand. */
async function tagsByUse(notes: NoteResponse[]): Promise<string[]> {
  const manual = new Set(await listManualTagNames());
  const uses = new Map<string, number>();
  for (const tag of notes.flatMap((n) => n.tags)) {
    if (manual.has(tag)) uses.set(tag, (uses.get(tag) ?? 0) + 1);
  }
  return [...uses]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([tag]) => tag);
}

/** The columns the notes table can show, marking where each note's text matches the search. */
function noteColumns(
  matches: Map<number, [number, number][]>,
  lastReviewed: Map<number, string | null>,
): Column<NoteResponse, NoteColumn>[] {
  const nowrap = { whiteSpace: "nowrap" } as const;
  return [
    {
      id: "id",
      label: "ID",
      sortField: "id",
      render: (note) => (
        <>
          {formatNoteId(note.id)}
          {note.tags.includes(SYNC_CONFLICT_TAG) && (
            <span
              className="sync-conflict-badge"
              title="A sync could not merge this note. Resolve it with `spares sync conflicts`."
              style={{ marginLeft: 6 }}
            >
              conflict
            </span>
          )}
        </>
      ),
    },
    {
      id: "data",
      label: "Data",
      sortField: "data",
      render: (note) => (
        <span title={note.data}>
          {highlightPreview(
            note.data,
            matches.get(note.id) ?? [],
            DATA_PREVIEW_LEN,
          ).map((part, i) =>
            part.mark ? (
              <mark key={i} className="search-match">
                {part.text}
              </mark>
            ) : (
              part.text
            ),
          )}
        </span>
      ),
    },
    {
      id: "tags",
      label: "Tags",
      sortField: "tag",
      render: (note) => note.tags.join(", ") || "—",
    },
    {
      id: "keywords",
      label: "Keywords",
      sortField: "keyword",
      hideNarrow: true,
      render: (note) => (
        <>
          {note.keywords.slice(0, 4).join(", ")}
          {note.keywords.length > 4 ? "…" : ""}
        </>
      ),
    },
    {
      id: "cards",
      label: "Cards",
      hideNarrow: true,
      render: (note) => note.card_count,
    },
    {
      id: "parser",
      label: "Parser",
      sortField: "parser_name",
      hideNarrow: true,
      render: (note) => note.parser_name,
    },
    {
      id: "reviewed",
      label: "Last reviewed",
      sortField: "c.reviewed",
      hideNarrow: true,
      tdStyle: nowrap,
      render: (note) => formatDate(lastReviewed.get(note.id)),
    },
    {
      id: "created",
      label: "Created",
      sortField: "created_at",
      hideNarrow: true,
      tdStyle: nowrap,
      render: (note) => formatDateTime(note.created_at),
    },
    {
      id: "updated",
      label: "Modified",
      sortField: "updated_at",
      hideNarrow: true,
      tdStyle: nowrap,
      render: (note) => formatDateTime(note.updated_at),
    },
  ];
}

export default function NotesPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.max(1, parseInt(searchParams.get("page") ?? "1", 10));
  const pageSizeChoice = useNotesPageSize();
  const pageSize = Number(pageSizeChoice);
  const [notes, setNotes] = useState<NoteResponse[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  // The search is kept in the URL so it survives navigating away and back
  const searchQuery = searchParams.get("q");
  const [query, setQuery] = useState(searchQuery ?? "");
  // The page of search results, how many there are in all, and where each note matches
  const [searchResults, setSearchResults] = useState<NoteResponse[] | null>(
    null,
  );
  const [searchTotal, setSearchTotal] = useState(0);
  const [searchMatches, setSearchMatches] = useState<
    Map<number, [number, number][]>
  >(new Map());
  // When each note's cards were last reviewed, for the pages read from a search
  const [lastReviewed, setLastReviewed] = useState<Map<number, string | null>>(
    new Map(),
  );
  const [searchError, setSearchError] = useState<SearchInputError | null>(null);
  const { sort, cycle: cycleSort } = useTableSort();
  const sortField = sort?.field ?? null;
  const sortDescending = sort?.descending ?? false;
  const shownColumnIds = useNoteColumns();
  // Without a search, the list is read from a search for every note when sorted or showing when
  // notes were last reviewed, which only a search returns
  const listBySearch =
    sortField !== null || shownColumnIds.includes("reviewed");
  // How many notes there are without a search, which that search says, or which are counted
  const [listTotal, setListTotal] = useState<number | null>(null);
  const allColumns = noteColumns(searchMatches, lastReviewed);
  const columns = allColumns.filter((c) => shownColumnIds.includes(c.id));
  const widths = useTableWidths("notes");
  const [selectedNote, setSelectedNote] = useState<NoteResponse | null>(null);
  const [creating, setCreating] = useState(false);
  // The new note form stays mounted, hidden, while other notes are viewed, so its unsaved
  // contents are still there when New note is clicked again
  const [hasDraft, setHasDraft] = useState(false);
  const [paneStatus, setPaneStatus] = useState<string | null>(null);
  // What the last import or delete did, numbered so each gets a fresh Undo button
  const [lastOutcome, setLastOutcome] = useState<{
    seq: number;
    outcome: ActionOutcome;
  } | null>(null);
  const showOutcome = (outcome: ActionOutcome) =>
    setLastOutcome((prev) => ({ seq: (prev?.seq ?? 0) + 1, outcome }));
  const [importOpen, setImportOpen] = useState(false);
  // Bumped after a bulk action, to refetch the notes it may have changed
  const [reloadCount, setReloadCount] = useState(0);
  // Fetch again after an undo, redo or sync elsewhere changed what is shown
  const dataVersion = useDataVersion();
  // Bumped when the shown note is changed by a bulk action, so its editor starts again from it
  const [detailVersion, setDetailVersion] = useState(0);
  // The notes seen on any page, to find the tags of those checked
  const seenNotes = useRef(new Map<number, NoteResponse>());
  const tableBody = useRef<HTMLTableSectionElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const notePane = useRef<NotePaneHandle>(null);
  const newNotePane = useRef<NotePaneHandle>(null);
  const rows = useRowSelection(searchQuery);

  useEffect(() => {
    if (!credentials) {
      navigate("/login");
      return;
    }
    if (searchQuery !== null || !listBySearch) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show loading state while the page is fetched
    setLoading(true);
    let cancelled = false;
    searchNotesPage(
      "",
      page,
      pageSize,
      sortField === null
        ? null
        : { field: sortField, descending: sortDescending },
    )
      .then((data) => {
        if (cancelled) return;
        setNotes(data.items.map((m) => m.note));
        setLastReviewed(
          new Map(data.items.map((m) => [m.note.id, m.last_reviewed])),
        );
        for (const { note } of data.items) seenNotes.current.set(note.id, note);
        setListTotal(data.total);
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [
    credentials,
    navigate,
    page,
    pageSize,
    searchQuery,
    reloadCount,
    dataVersion,
    listBySearch,
    sortField,
    sortDescending,
  ]);

  useEffect(() => {
    if (!credentials) {
      navigate("/login");
      return;
    }
    if (searchQuery !== null || listBySearch) return;
    // Show the page from an earlier visit straight away while it is refetched
    const cached = cachedNotesPage(page, pageSize);
    // eslint-disable-next-line react-hooks/set-state-in-effect -- show the cached page while the page is fetched
    if (cached) setNotes(cached);
    setLoading(cached === undefined);
    let cancelled = false;
    // Until it is counted, the last page is the one with fewer notes than a page has
    countNotes().then(
      (total) => {
        if (!cancelled) setListTotal(total);
      },
      () => {},
    );
    listNotes(page, pageSize)
      .then((data) => {
        if (cancelled) return;
        setNotes(data);
        for (const note of data) seenNotes.current.set(note.id, note);
        setError(null);
        // Prefetch the next page so Next shows it straight away
        if (data.length === pageSize)
          listNotes(page + 1, pageSize).catch(() => {});
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [
    credentials,
    navigate,
    page,
    pageSize,
    searchQuery,
    reloadCount,
    dataVersion,
    listBySearch,
  ]);

  const {
    recent: recentSearches,
    save: saveRecentSearch,
    remove: removeRecentSearch,
  } = useRecentSearches("spares_notes_recent_searches");
  // Bumped by each search so searching the same query again refetches it
  const [searchCount, setSearchCount] = useState(0);

  useEffect(() => {
    if (!credentials) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- the input follows the URL, e.g. on back
    setQuery(searchQuery ?? "");
    if (searchQuery === null) {
      setSearchResults(null);
      return;
    }
    setLoading(true);
    setError(null);
    let cancelled = false;
    searchNotesPage(
      searchQuery,
      page,
      pageSize,
      sortField === null
        ? null
        : { field: sortField, descending: sortDescending },
    )
      .then((data) => {
        if (cancelled) return;
        setSearchResults(data.items.map((m) => m.note));
        for (const { note } of data.items) seenNotes.current.set(note.id, note);
        setSearchTotal(data.total);
        setSearchMatches(
          new Map(data.items.map((m) => [m.note.id, m.matches])),
        );
        setLastReviewed(
          new Map(data.items.map((m) => [m.note.id, m.last_reviewed])),
        );
        saveRecentSearch(searchQuery);
        setError(null);
        setSearchError(null);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(errorMessage(e));
        setSearchError(
          e instanceof ApiError && e.span
            ? { query: searchQuery, span: e.span }
            : null,
        );
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [
    credentials,
    searchQuery,
    page,
    pageSize,
    searchCount,
    reloadCount,
    dataVersion,
    saveRecentSearch,
    sortField,
    sortDescending,
  ]);

  function handleSearch(q = query) {
    if (!q.trim()) return;
    setQuery(q);
    setSearchParams((prev) => withPage(withSearch(prev, q), null));
    setSearchCount((n) => n + 1);
  }

  const savedSearches = useSavedSearches();

  function handleSaveSearch() {
    const q = query.trim();
    if (!q) return;
    const name = window.prompt(
      "Save this search as:",
      savedSearches.saved.find((s) => s.query === q)?.name ?? "",
    );
    if (name?.trim()) void savedSearches.save(name.trim(), q);
  }

  function startNewNote() {
    setSelectedNote(null);
    setPaneStatus(null);
    setCreating(true);
    setHasDraft(true);
    requestAnimationFrame(() => newNotePane.current?.focus());
  }

  function handleClear() {
    setSearchParams((prev) => withPage(withSearch(prev, null), null));
  }

  const pageCount = Math.max(
    1,
    Math.ceil(
      (searchResults !== null ? searchTotal : (listTotal ?? 0)) / pageSize,
    ),
  );
  const hasNextPage =
    searchResults !== null || listTotal !== null
      ? page < pageCount
      : notes.length === pageSize;

  const displayedNotes = searchResults ?? notes;
  // -1 if the selected note was opened from a link and isn't in the table
  const selectedIndex = selectedNote
    ? displayedNotes.findIndex((n) => n.id === selectedNote.id)
    : -1;

  function selectNote(note: NoteResponse | null) {
    setSelectedNote(note);
    setCreating(false);
    setPaneStatus(null);
  }

  const detail = useDetailView(selectedNote !== null || creating, () =>
    selectNote(null),
  );

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
    try {
      const eventId = await deleteNotes({ Ids: [id] });
      setNotes((prev) => prev.filter((n) => n.id !== id));
      if (searchResults?.some((n) => n.id === id))
        setSearchTotal((total) => total - 1);
      if (notes.some((n) => n.id === id))
        setListTotal((total) => (total === null ? null : total - 1));
      setSearchResults((prev) =>
        prev ? prev.filter((n) => n.id !== id) : prev,
      );
      setSelectedNote(null);
      showOutcome({
        message: `Deleted note ${id} and its cards.`,
        eventIds: eventId === null ? [] : [eventId],
      });
    } catch (e) {
      setPaneStatus(errorMessage(e));
    }
  }

  /** The notes `selection` has, for the tags they have. */
  async function selectedNotes(
    selection: RowSelection,
  ): Promise<NoteResponse[]> {
    if (!selection.all)
      return [...selection.ids].flatMap(
        (id) => seenNotes.current.get(id) ?? [],
      );
    if (searchQuery === null) return [];
    return (await searchNotes(searchQuery)).filter(
      (n) => !selection.ids.has(n.id),
    );
  }

  async function tagNotes(
    selector: IdsOrQuery,
    add: string[],
    remove: string[],
  ) {
    const { notes: changed, eventId } = await modifyNoteTags(
      selector,
      add,
      remove,
    );
    const message = add.length
      ? `Added ${tagList(add)} to ${changed.length} note(s).`
      : `Removed ${tagList(remove)} from ${changed.length} note(s).`;
    return { message, eventIds: eventId === null ? [] : [eventId] };
  }

  const bulkActions: BulkAction[] = [
    {
      label: "Add tags",
      input: { kind: "tags", suggestions: listManualTagNames },
      run: (selector, tags) => tagNotes(selector, tags, []),
    },
    {
      label: "Remove tags",
      input: {
        kind: "tags",
        suggestions: async () => tagsByUse(await selectedNotes(rows.selection)),
      },
      run: (selector, tags) => tagNotes(selector, [], tags),
    },
    {
      label: "Change parser",
      input: {
        kind: "choice",
        options: async () =>
          (await listParsers(1, 1_000)).map((p) => ({
            value: p.name,
            label: p.name,
          })),
      },
      run: async (selector, [name]) => {
        const parser = (await listParsers(1, 1_000)).find(
          (p) => p.name === name,
        );
        if (!parser) throw new Error(`No parser named ${name}`);
        const { notes: changed, eventId } = await changeNotesParser(
          selector,
          parser.id,
        );
        return {
          message: `Changed the parser of ${changed.length} note(s) to ${name}.`,
          eventIds: eventId === null ? [] : [eventId],
        };
      },
    },
    {
      label: "Export",
      input: {
        kind: "choice",
        options: async () => [
          { value: "share", label: "To share" },
          { value: "edit", label: "To edit and import back" },
        ],
        initial: "share",
      },
      run: async (selector, [mode], count) => {
        const query = queryOf(selector);
        if (mode === "edit") {
          const files = Object.entries(await exportNotes(query));
          for (const [extension, text] of files)
            downloadText(`export.${extension}`, text);
          return {
            message: `Exported ${count} note(s) to ${files.map(([extension]) => `export.${extension}`).join(", ") || "no files"}, to import back with \`spares import\`.`,
            eventIds: [],
          };
        }
        const summary = shareSummary(await fetchShareSummary(query), count);
        if (!window.confirm(`${summary}\n\nDownload shared.zip?`))
          return { message: "Not exported.", eventIds: [] };
        downloadBlob("shared.zip", await shareBundle(query));
        return { message: `Exported to shared.zip. ${summary}`, eventIds: [] };
      },
    },
    FILTERED_TAG_ACTION,
    {
      label: "Delete",
      danger: true,
      run: async (selector, _, count) => {
        const eventId = await deleteNotes(selector);
        return {
          message: `Deleted ${count} note(s).`,
          eventIds: eventId === null ? [] : [eventId],
        };
      },
    },
  ];

  /** Shows what a bulk action, or undoing it, changed. */
  function afterBulkAction() {
    rows.reset();
    setReloadCount((n) => n + 1);
    if (!selectedNote) return;
    getNote(selectedNote.id).then(
      (note) => {
        setSelectedNote(note);
        setDetailVersion((v) => v + 1);
      },
      (e: unknown) => {
        if (!isNetworkError(e)) setSelectedNote(null);
      },
    );
  }

  const header = rows.header(displayedNotes);
  const goToPage = (n: number) => setSearchParams((prev) => withPage(prev, n));
  const prevPage = page > 1 ? () => goToPage(page - 1) : null;
  const nextPage = hasNextPage ? () => goToPage(page + 1) : null;
  /** Lists `size` notes on each page, from the page with the first note of this one. */
  function changePageSize(size: PageSize) {
    setSearchParams((prev) =>
      withPage(prev, Math.floor(((page - 1) * pageSize) / Number(size)) + 1),
    );
    setNotesPageSize(size);
  }
  const keys = useListKeys({
    rows,
    displayed: displayedNotes,
    openIndex: selectedIndex,
    open: (i) => selectNote(displayedNotes[i]),
    close: creating
      ? () => newNotePane.current?.close()
      : selectedNote
        ? () => notePane.current?.close()
        : null,
    edit: {
      description: "Go to the open note's text",
      run:
        creating || selectedNote
          ? () => (creating ? newNotePane : notePane).current?.focus()
          : null,
    },
    prevPage,
    nextPage,
    searching: searchQuery !== null,
    body: tableBody,
    search: searchInput,
    extra: [
      {
        keys: ["c"],
        codes: ["c"],
        description: "Write a new note",
        available: !creating,
        run: startNewNote,
      },
      // A Mac's Delete key is Backspace
      {
        keys: ["Delete", "#"],
        codes: ["Delete", "Backspace", "#"],
        description: "Delete the open note (Backspace on a Mac)",
        available: selectedNote !== null && !creating,
        run: () => void deleteSelected(),
      },
    ],
  });

  return (
    <div className="page-wide">
      <style>{`
        .notes-row:hover { background-color: var(--hover); }
        .notes-row-selected { background-color: var(--selected) !important; }
      `}</style>

      <NavbarHelp help={keys} />
      <PageTitle>Notes</PageTitle>

      <div className="split">
        {/* Hidden rather than unmounted on phones, so it is unchanged when going back to it */}
        <div
          className="split-list"
          style={{
            flex: 1,
            minWidth: 0,
            display: detail.showList ? undefined : "none",
          }}
        >
          <div
            className="search-row"
            style={{
              display: "flex",
              gap: 8,
              marginBottom:
                recentSearches.length + savedSearches.saved.length > 0 ? 8 : 16,
            }}
          >
            <SearchInput
              ref={searchInput}
              value={query}
              onChange={setQuery}
              onSubmit={handleSearch}
              placeholder="Search notes…"
              items="Notes"
              error={searchError}
              style={{ maxWidth: "none" }}
              inputStyle={{ fontSize: 15, padding: "8px 12px" }}
            />
            <button
              onClick={() => handleSearch()}
              className="touch-target-small"
            >
              Search
            </button>
            <button
              onClick={handleSaveSearch}
              disabled={!query.trim()}
              className="touch-target-small"
              title="Save this search, for here and `spares note search --saved`"
            >
              Save
            </button>
            {searchResults !== null && (
              <button onClick={handleClear} className="touch-target-small">
                Clear
              </button>
            )}
            <Link
              to={
                searchQuery
                  ? `/graph?q=${encodeURIComponent(searchQuery)}`
                  : "/graph"
              }
              className="hide-narrow"
              style={{ alignSelf: "center", fontSize: 13 }}
              title="See how these notes link to the notes they build on"
            >
              Graph
            </Link>
            <button
              onClick={() => setImportOpen(true)}
              className="touch-target-small"
              style={{ marginLeft: "auto" }}
              title="Import files of notes, or zips of shared notes"
            >
              Import
            </button>
            {importOpen && (
              <ImportDialog
                onImported={(outcome) => {
                  showOutcome(outcome);
                  afterBulkAction();
                }}
                onClose={() => setImportOpen(false)}
              />
            )}
            <button onClick={startNewNote} className="touch-target-small">
              New note
            </button>
          </div>
          {lastOutcome && (
            <ActionResult
              key={lastOutcome.seq}
              outcome={lastOutcome.outcome}
              onReverted={afterBulkAction}
            />
          )}
          <SavedSearches searches={savedSearches} onSelect={handleSearch} />
          <RecentSearches
            recent={recentSearches}
            onSelect={handleSearch}
            onRemove={removeRecentSearch}
            style={{ marginBottom: 16 }}
          />

          <div
            style={{
              display: "flex",
              gap: 12,
              alignItems: "center",
              flexWrap: "wrap",
              fontSize: 13,
              color: "var(--text-secondary)",
              marginBottom: 8,
            }}
          >
            {searchResults !== null && (
              <span>
                {searchTotal} result{searchTotal !== 1 ? "s" : ""} for "
                {searchQuery}"
              </span>
            )}
            <span style={{ marginLeft: "auto" }}>
              <ColumnPicker
                columns={allColumns}
                shown={shownColumnIds}
                onChange={setNoteColumns}
              />
            </span>
          </div>

          <BulkActionBar
            noun="note"
            selection={rows.selection}
            query={searchQuery}
            total={searchTotal}
            onSelectAll={() => rows.selectAll(displayedNotes)}
            onSelectNone={rows.selectNone}
            onInvert={() => rows.invert(displayedNotes)}
            search={async (q) => (await searchNotes(q)).map((n) => n.id)}
            actions={bulkActions}
            onDone={afterBulkAction}
          />

          {error && (
            <pre
              style={{
                color: "var(--error)",
                marginBottom: 12,
                marginTop: 0,
                whiteSpace: "pre-wrap",
              }}
            >
              {error}
            </pre>
          )}
          {loading && <div>Loading…</div>}

          {!loading && (
            <div className="table-scroll">
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr>
                    <th style={{ ...th, width: 1, paddingRight: 0 }}>
                      <Checkbox
                        checked={header.checked}
                        indeterminate={header.indeterminate}
                        onClick={header.toggle}
                        label={
                          searchQuery !== null
                            ? "Select all notes found"
                            : "Select the notes on this page"
                        }
                      />
                    </th>
                    {columns.map((column) => (
                      <ColumnHeader
                        key={column.id}
                        column={column}
                        sort={sort}
                        onSort={cycleSort}
                        widths={widths}
                      />
                    ))}
                  </tr>
                </thead>
                <tbody ref={tableBody}>
                  {displayedNotes.map((note, i) => (
                    <tr
                      key={note.id}
                      className={`notes-row${selectedNote?.id === note.id ? " notes-row-selected" : ""}`}
                      onClick={() =>
                        selectNote(selectedNote?.id === note.id ? null : note)
                      }
                      style={{ cursor: "pointer" }}
                    >
                      <td
                        style={checkTd}
                        onClick={(e) => {
                          e.stopPropagation();
                          rows.toggleRow(displayedNotes, i, e.shiftKey);
                        }}
                      >
                        <Checkbox
                          checked={rows.isSelected(note.id)}
                          onClick={(e) =>
                            rows.toggleRow(displayedNotes, i, e.shiftKey)
                          }
                          label={`Select note ${note.id}`}
                        />
                      </td>
                      {columns.map((column) => {
                        const className =
                          [
                            column.hideNarrow && "hide-narrow",
                            column.id === "data" && "clamp-narrow",
                          ]
                            .filter(Boolean)
                            .join(" ") || undefined;
                        const style =
                          column.id === "data"
                            ? dataTd
                            : column.tdStyle
                              ? { ...td, ...column.tdStyle }
                              : td;
                        return (
                          <td
                            key={column.id}
                            style={widths.style(column.id, style)}
                            className={className}
                          >
                            {column.render(note)}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                  {displayedNotes.length === 0 && (
                    <tr>
                      <td
                        colSpan={columns.length + 1}
                        style={{
                          ...td,
                          color: "var(--text-muted)",
                          textAlign: "center",
                        }}
                      >
                        {searchResults === null && page === 1 ? (
                          <>
                            No notes yet.{" "}
                            <button
                              onClick={startNewNote}
                              className="button-small"
                            >
                              Write one
                            </button>
                            , or press <kbd>?</kbd> to learn how.
                          </>
                        ) : (
                          "No notes found"
                        )}
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </div>
          )}

          <Pagination
            page={page}
            pageCount={
              searchResults !== null || listTotal !== null ? pageCount : null
            }
            prevPage={prevPage}
            nextPage={nextPage}
            goToPage={goToPage}
            pageSize={pageSizeChoice}
            setPageSize={changePageSize}
          />
        </div>

        <div className="split-detail" style={{ flex: 1, minWidth: 0 }}>
          {!detail.showList && (
            <button
              onClick={detail.back}
              className="touch-target"
              style={{ marginBottom: 12 }}
            >
              ← Back to notes
            </button>
          )}
          {paneStatus && (
            <div
              style={{
                fontSize: 13,
                color: "var(--text-secondary)",
                marginBottom: 8,
              }}
            >
              {paneStatus}
            </div>
          )}
          {hasDraft && (
            <div style={{ display: creating ? undefined : "none" }}>
              <NewNoteForm
                ref={newNotePane}
                onCreated={(note) => {
                  setHasDraft(false);
                  selectNote(note);
                  setPaneStatus(`Note ${formatNoteId(note.id)} created.`);
                }}
                onCancel={() => {
                  setHasDraft(false);
                  detail.back();
                }}
              />
            </div>
          )}
          {creating ? null : selectedNote ? (
            <>
              <div
                style={{
                  display: "flex",
                  gap: 8,
                  alignItems: "center",
                  flexWrap: "wrap",
                  marginBottom: 8,
                  fontSize: 13,
                }}
              >
                {selectedIndex >= 0 && (
                  <>
                    {/* Wraps around at either end, as in the CLI */}
                    <button
                      onClick={() =>
                        selectNote(
                          displayedNotes[
                            (selectedIndex + displayedNotes.length - 1) %
                              displayedNotes.length
                          ],
                        )
                      }
                      disabled={displayedNotes.length < 2}
                      className="touch-target-small"
                    >
                      Previous
                    </button>
                    <span>
                      Note {selectedIndex + 1} of {displayedNotes.length}
                    </span>
                    <button
                      onClick={() =>
                        selectNote(
                          displayedNotes[
                            (selectedIndex + 1) % displayedNotes.length
                          ],
                        )
                      }
                      disabled={displayedNotes.length < 2}
                      className="touch-target-small"
                    >
                      Next
                    </button>
                  </>
                )}
                <Link
                  to={`/graph?q=${encodeURIComponent(`id=${formatNoteId(selectedNote.id)}`)}`}
                  style={{ marginLeft: "auto" }}
                  title="See the notes this note builds on"
                >
                  Show in graph
                </Link>
                <button
                  onClick={deleteSelected}
                  className="touch-target-small"
                  style={{ color: "var(--danger)" }}
                >
                  Delete note
                </button>
              </div>
              <NoteDetail
                ref={notePane}
                key={`${selectedNote.id}:${detailVersion}`}
                note={selectedNote}
                onClose={detail.back}
                onNoteUpdated={(updated) => {
                  setSelectedNote((prev) => keepShown(prev, updated));
                  setNotes((prev) =>
                    prev.map((n) => (n.id === updated.id ? updated : n)),
                  );
                  setSearchResults((prev) =>
                    prev
                      ? prev.map((n) => (n.id === updated.id ? updated : n))
                      : prev,
                  );
                }}
                onOpenNote={openLinkedNote}
              />
            </>
          ) : (
            !detail.isNarrow && (
              <div
                style={{
                  color: "var(--text-faint)",
                  fontSize: 14,
                  paddingTop: 8,
                }}
              >
                Select a note to see details.
              </div>
            )
          )}
        </div>
      </div>
    </div>
  );
}
