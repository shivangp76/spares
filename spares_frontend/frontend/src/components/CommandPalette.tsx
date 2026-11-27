import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  type Command,
  looksLikeNoteId,
  currentPageCommands,
  matchCommands,
  startsAWord,
} from "../commandPalette";
import { useSavedSearches } from "../hooks/useSavedSearches";
import { NAV_LINKS } from "../navLinks";

/**
 * Ctrl+K (Cmd+K on a Mac), from any page: what is typed opens a note by its id, searches notes or
 * cards, starts a review or shows the graph, and the list below goes to a page, runs an action or a
 * saved search. ↑ and ↓ pick, as do Ctrl+J and Ctrl+K or Ctrl+N and Ctrl+P as in Vim, Enter runs,
 * Esc closes. Opened by the navbar, which gives it the
 * actions only it can do.
 */
export default function CommandPalette({
  onClose,
  onUndo,
  onHelp,
  onCapture,
}: {
  onClose: () => void;
  onUndo: () => void;
  /** Opens the page's help, where it has one. */
  onHelp?: () => void;
  /** Writes a new note over the page (`QuickCapture`), where it can */
  onCapture?: () => void;
}) {
  const navigate = useNavigate();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const listId = useId();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const { saved } = useSavedSearches();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  const commands = useMemo(() => {
    const q = query.trim();
    const go = (to: string) => () => navigate(to);
    const enc = encodeURIComponent;
    // What is typed, always first
    const typed: Command[] = q
      ? [
          ...(looksLikeNoteId(q)
            ? [
                {
                  id: "open-note",
                  label: `Open note ${q.toLowerCase()}`,
                  hint: "Note",
                  run: go(`/notes?note=${enc(q.toLowerCase())}`),
                },
              ]
            : []),
          {
            id: "search-notes",
            label: `Search notes for ${q}`,
            hint: "Notes",
            run: go(`/notes?q=${enc(q)}`),
          },
          {
            id: "search-cards",
            label: `Search cards for ${q}`,
            hint: "Cards",
            run: go(`/cards?q=${enc(q)}`),
          },
          {
            id: "review-query",
            label: `Review the cards due that ${q} finds`,
            hint: "Review",
            run: go(`/review?query=${enc(q)}&start`),
          },
          {
            id: "graph-query",
            label: `Show ${q} in the graph`,
            hint: "Graph",
            run: go(`/graph?q=${enc(q)}`),
          },
        ]
      : [];
    // The navbar's Sync button, which knows whether and how this device syncs
    const syncButton = document.querySelector<HTMLButtonElement>(
      "[data-sync-button]",
    );
    const listed: Command[] = [
      {
        id: "new-note",
        label: "New note",
        hint: "c",
        // Over this page, or on the Notes page in its pane
        run: onCapture ?? go("/notes?new"),
      },
      {
        id: "review",
        label: "Start reviewing every due card",
        hint: "Review",
        run: go("/review?query=&start"),
      },
      { id: "undo", label: "Undo the last change", hint: "Undo", run: onUndo },
      ...currentPageCommands(),
      ...(syncButton && !syncButton.disabled
        ? [
            {
              id: "sync",
              label: "Sync",
              hint: "Sync",
              run: () => syncButton.click(),
            },
          ]
        : []),
      ...(onHelp
        ? [
            {
              id: "help",
              label: "Help for this page",
              hint: "?",
              run: onHelp,
            },
          ]
        : []),
      ...NAV_LINKS.map((link) => ({
        id: `go-${link.to}`,
        label: `Go to ${link.label}`,
        hint: `g ${link.key}`,
        run: go(link.to),
      })),
      ...saved.flatMap((search) => [
        {
          id: `review-saved-${search.name}`,
          label: `Review ${search.name}`,
          hint: "Saved search",
          run: go(`/review?query=${enc(search.query)}&start`),
        },
        {
          id: `notes-saved-${search.name}`,
          label: `Notes in ${search.name}`,
          hint: "Saved search",
          run: go(`/notes?q=${enc(search.query)}`),
        },
      ]),
    ];
    const matched = matchCommands(listed, q);
    // A query that names a command puts it above the searches for the text itself
    return matched.length > 0 && startsAWord(matched[0].label, q)
      ? [...matched, ...typed]
      : [...typed, ...matched];
  }, [query, saved, navigate, onUndo, onHelp, onCapture]);

  const shown = Math.min(active, Math.max(0, commands.length - 1));

  // Keeps the highlighted entry in view as ↑ and ↓ move it
  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${shown}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [shown]);

  function run(command: Command | undefined) {
    if (!command) return;
    dialogRef.current?.close();
    command.run();
  }

  return (
    <dialog
      ref={dialogRef}
      className="help-dialog command-palette"
      aria-label="Command palette"
      onClose={onClose}
      onClick={(e) => {
        if (e.target === e.currentTarget) dialogRef.current?.close();
      }}
    >
      <input
        className="command-palette-input"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
        }}
        onKeyDown={(e) => {
          // Vim's keys for a completion list, besides the arrows
          const ctrl = e.ctrlKey || e.metaKey;
          const down =
            e.key === "ArrowDown" || (ctrl && (e.key === "j" || e.key === "n"));
          const up =
            e.key === "ArrowUp" || (ctrl && (e.key === "k" || e.key === "p"));
          if (down) {
            e.preventDefault();
            setActive(Math.min(shown + 1, commands.length - 1));
          } else if (up) {
            e.preventDefault();
            setActive(Math.max(shown - 1, 0));
          } else if (e.key === "Enter") {
            e.preventDefault();
            run(commands[shown]);
          }
        }}
        placeholder="A note id, a search, or a page or action…"
        autoFocus
        role="combobox"
        aria-expanded="true"
        aria-controls={listId}
        aria-activedescendant={`${listId}-${shown}`}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
      />
      <ul ref={listRef} id={listId} role="listbox" className="command-palette-list">
        {commands.map((command, i) => (
          <li
            key={command.id}
            id={`${listId}-${i}`}
            data-index={i}
            role="option"
            aria-selected={i === shown}
            className={i === shown ? "active" : undefined}
            onMouseMove={() => setActive(i)}
            onClick={() => run(command)}
          >
            <span>{command.label}</span>
            {command.hint && <span className="key-hint">{command.hint}</span>}
          </li>
        ))}
        {commands.length === 0 && (
          <li className="command-palette-empty">Nothing matches.</li>
        )}
      </ul>
    </dialog>
  );
}
