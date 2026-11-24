import { useEffect, useImperativeHandle, useMemo, useRef, useState } from "react";
import CodeMirror, { type ReactCodeMirrorRef } from "@uiw/react-codemirror";
import { createNotes, listParsers, renderNote } from "../api/client";
import { EDITOR_TIPS, useEditorSetup } from "../hooks/useEditorSetup";
import { useParserSyntax } from "../hooks/useParserSyntax";
import { submitKeymap, useSubmitKeys } from "../hooks/useSubmitKeys";
import {
  draftTime,
  newNoteDefaults,
  newNoteDraft,
  useDraftSaver,
} from "../noteDrafts";
import { useVimKeybindings } from "../preferences";
import { useResolvedTheme } from "../theme";
import type { NoteResponse, ParserResponse } from "../types/spares";
import {
  errorMessage,
  formatNoteId,
  insertBlock,
  insertInline,
  insertSource,
} from "../utils";
import { NO_WRITE_WARNING, useVimCommands } from "../vimCommands";
import InsertAudioButton from "./InsertAudioButton";
import InsertImageButton from "./InsertImageButton";
import InsertImageOcclusionButton from "./InsertImageOcclusionButton";
import type { NotePaneHandle } from "./NoteDetail";
import NoteSyntaxButton from "./NoteSyntaxHelp";
import { askConfirm, showNotice } from "../ask";

const fieldLabel: React.CSSProperties = {
  fontSize: 12,
  color: "var(--text-muted)",
  marginBottom: 4,
  fontWeight: 600,
  textTransform: "uppercase",
  letterSpacing: "0.05em",
};
const cmStyle = {
  border: "1px solid var(--border-subtle)",
  borderRadius: 4,
  background: "var(--bg)",
};
// Enough to list every parser on one page
const PARSERS_LIMIT = 1000;
const VIM_TIP =
  "With Vim keybindings, :w, :wq or :x creates the note, and :q cancels it (:q! drops what has been typed). :e! clears the form.";

function lines(value: string): string[] {
  return value
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** What a new note starts with, as "New note like this" gives it from another note. */
export interface NewNoteStart {
  parserId?: number;
  /** The parser by name, where its id isn't known, as on the Review page. */
  parserName?: string;
  tags?: string[];
  keywords?: string[];
}

/**
 * `spares note add`, followed by rendering the note as the CLI does when syncing it. What is typed
 * is kept in this browser until the note is created or discarded, and restored if the page is
 * reloaded. Without `start`, a note starts with the parser and tags of the last one created here.
 */
export default function NewNoteForm({
  onCreated,
  onCancel,
  start,
  ref,
}: {
  /** `more` when Create and New was used, which keeps the form open for another note. */
  onCreated: (note: NoteResponse, more: boolean) => void;
  onCancel: () => void;
  start?: NewNoteStart;
  ref?: React.Ref<NotePaneHandle>;
}) {
  // Read once: a draft left from before, else `start`, else the last note's parser and tags
  const [initial] = useState(() => {
    const draft = start ? null : newNoteDraft.load();
    const defaults = newNoteDefaults.load();
    const tags = start?.tags?.join("\n") ?? defaults?.tags ?? "";
    const keywords = start?.keywords?.join("\n") ?? "";
    return {
      parserId: draft?.value.parserId ?? start?.parserId ?? defaults?.parserId,
      // What the fields start as when nothing has been typed, to tell typed text apart
      tags,
      keywords,
      draft,
    };
  });
  const [restoredAt, setRestoredAt] = useState(initial.draft?.savedAt ?? null);
  const theme = useResolvedTheme();
  const [parsers, setParsers] = useState<ParserResponse[] | null>(null);
  const [parserId, setParserId] = useState<number | null>(null);
  const editor = useEditorSetup(
    parsers?.find((p) => p.id === parserId)?.name,
    parserId,
  );
  const vimEnabled = useVimKeybindings();
  const [data, setData] = useState(initial.draft?.value.data ?? "");
  const dataEditorRef = useRef<ReactCodeMirrorRef>(null);
  const [tags, setTags] = useState(initial.draft?.value.tags ?? initial.tags);
  const [keywords, setKeywords] = useState(
    initial.draft?.value.keywords ?? initial.keywords,
  );
  const [isSuspended, setIsSuspended] = useState(
    initial.draft?.value.isSuspended ?? false,
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // What the tags and keywords hold before anything is typed: prefilled ones are nothing to lose
  const [baseline, setBaseline] = useState({
    tags: initial.tags,
    keywords: initial.keywords,
  });
  const unsaved =
    data.trim() !== "" ||
    tags.trim() !== baseline.tags.trim() ||
    keywords.trim() !== baseline.keywords.trim();
  useDraftSaver(
    unsaved ? { parserId, data, tags, keywords, isSuspended } : null,
    newNoteDraft.save,
  );
  // The first example shows what to write while the data is empty
  const firstExample = useParserSyntax(parserId).examples?.[0]?.source;

  useEffect(() => {
    listParsers(1, PARSERS_LIMIT)
      .then((list) => {
        setParsers(list);
        // The remembered parser, unless it was deleted since
        const remembered =
          list.find((p) => p.id === initial.parserId) ??
          list.find((p) => p.name === start?.parserName);
        setParserId(remembered?.id ?? list[0]?.id ?? null);
      })
      .catch((e) => setError(errorMessage(e)));
    // Once, with the parser remembered when the form opened
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Empties the note's text for another note, keeping its parser and `keptTags`. */
  function clearText(keptTags: string) {
    setData("");
    setTags(keptTags);
    setKeywords(initial.keywords);
    setBaseline({ tags: keptTags, keywords: initial.keywords });
    setRestoredAt(null);
    setError(null);
  }

  async function cancel() {
    if (
      !unsaved ||
      (await askConfirm({
        title: "Discard this new note?",
        confirmLabel: "Discard",
        danger: true,
      }))
    ) {
      newNoteDraft.save(null);
      onCancel();
    }
  }

  const dataExtensions = useMemo(
    () => [...editor.dataExtensions, submitKeymap],
    [editor.dataExtensions],
  );
  const fieldExtensions = useMemo(
    () => [...editor.extensions, submitKeymap],
    [editor.extensions],
  );

  useImperativeHandle(ref, () => ({
    focus: () => dataEditorRef.current?.view?.focus(),
    close: cancel,
  }));

  const rootRef = useRef<HTMLDivElement>(null);
  useSubmitKeys(rootRef, (more) => {
    if (!saving) void create(more);
    return true;
  });
  // Creating the note closes the form, so each way of writing it also quits
  useVimCommands(rootRef, ({ command, force, warn }) => {
    if (command === "edit") {
      if (force) {
        setIsSuspended(false);
        clearText(baseline.tags);
      } else if (unsaved) {
        warn(NO_WRITE_WARNING);
      }
    } else if (command === "quit" || (command === "exit" && !unsaved)) {
      if (force || !unsaved) onCancel();
      else warn(NO_WRITE_WARNING);
    } else if (!saving) {
      void create(false);
    }
  });

  /** Creates the note; with `more`, keeps the form open, emptied, for another like it. */
  async function create(more: boolean) {
    if (parserId === null) {
      setError("Select a parser");
      return;
    }
    if (!data.trim()) {
      setError("Data is empty");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const [note] = await createNotes(parserId, [
        {
          data,
          keywords: lines(keywords),
          tags: lines(tags),
          is_suspended: isSuspended,
          custom_data: {},
        },
      ]);
      newNoteDraft.save(null);
      newNoteDefaults.save({ parserId, tags });
      try {
        await renderNote(note.id);
      } catch (e) {
        // The note exists, so still hand it over
        void showNotice({
          title: `Note ${formatNoteId(note.id)} created`,
          message: `But ${errorMessage(e)}`,
        });
      }
      if (more) {
        clearText(tags);
        dataEditorRef.current?.view?.focus();
      }
      onCreated(note, more);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      ref={rootRef}
      style={{
        border: "1px solid var(--border)",
        borderRadius: 6,
        padding: 20,
        backgroundColor: "var(--surface)",
      }}
    >
      <h3 style={{ marginTop: 0, marginBottom: 16, fontSize: 16 }}>New Note</h3>
      {restoredAt !== null && (
        <div className="draft-notice" role="status">
          <span>Restored a draft from {draftTime(restoredAt)}.</span>
          <button
            type="button"
            className="button-small"
            onClick={() => {
              setIsSuspended(false);
              clearText(initial.tags);
            }}
          >
            Discard it
          </button>
        </div>
      )}

      <div style={{ marginBottom: 16 }}>
        <div style={fieldLabel}>Parser</div>
        <select
          value={parserId ?? ""}
          onChange={(e) => setParserId(Number(e.target.value))}
          disabled={!parsers}
          style={{ padding: "6px 10px", fontSize: 14 }}
        >
          {!parsers && <option>Loading…</option>}
          {parsers?.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} ({p.id})
            </option>
          ))}
        </select>
      </div>

      <div style={{ marginBottom: 16 }}>
        <div
          style={{
            display: "flex",
            alignItems: "baseline",
            justifyContent: "space-between",
            gap: 8,
          }}
        >
          <div style={fieldLabel}>Data</div>
          {/* Wraps on phones rather than pushing the page sideways */}
          <div
            style={{
              display: "flex",
              gap: 6,
              flexWrap: "wrap",
              justifyContent: "flex-end",
            }}
          >
            <NoteSyntaxButton
              parserId={parserId}
              tips={vimEnabled ? [...EDITOR_TIPS, VIM_TIP] : EDITOR_TIPS}
              onInsert={(source) =>
                setData(insertSource(dataEditorRef.current?.view, data, source))
              }
            />
            <InsertAudioButton
              parserId={parserId}
              onInsert={(snippet) =>
                setData(
                  insertInline(dataEditorRef.current?.view, data, snippet),
                )
              }
            />
            <InsertImageButton
              parserId={parserId}
              view={() => dataEditorRef.current?.view}
            />
            <InsertImageOcclusionButton
              parserId={parserId}
              onInsert={(snippet) =>
                setData(insertBlock(dataEditorRef.current?.view, data, snippet))
              }
            />
          </div>
        </div>
        <CodeMirror
          ref={dataEditorRef}
          theme={theme}
          value={data}
          onChange={setData}
          placeholder={firstExample && `e.g. ${firstExample}`}
          extensions={dataExtensions}
          basicSetup={editor.dataSetup}
          minHeight="120px"
          style={cmStyle}
        />
      </div>

      <div className="form-grid">
        <div>
          <div style={fieldLabel}>
            Tags{" "}
            <span
              style={{
                fontWeight: 400,
                textTransform: "none",
                letterSpacing: 0,
              }}
            >
              (one per line)
            </span>
          </div>
          <CodeMirror
            theme={theme}
            value={tags}
            onChange={setTags}
            extensions={fieldExtensions}
            basicSetup={editor.fieldSetup}
            style={cmStyle}
          />
        </div>
        <div>
          <div style={fieldLabel}>
            Keywords{" "}
            <span
              style={{
                fontWeight: 400,
                textTransform: "none",
                letterSpacing: 0,
              }}
            >
              (one per line)
            </span>
          </div>
          <CodeMirror
            theme={theme}
            value={keywords}
            onChange={setKeywords}
            extensions={fieldExtensions}
            basicSetup={editor.fieldSetup}
            style={cmStyle}
          />
        </div>
      </div>

      <label style={{ display: "block", fontSize: 13, marginBottom: 16 }}>
        <input
          type="checkbox"
          checked={isSuspended}
          onChange={(e) => setIsSuspended(e.target.checked)}
          style={{ marginRight: 6 }}
        />
        Suspend its cards
      </label>

      <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
        <button
          onClick={() => create(false)}
          disabled={saving || parserId === null}
          className="button-primary touch-target-small"
          style={{ padding: "5px 14px", fontSize: 13 }}
          title="Ctrl+Enter (Cmd+Enter on a Mac)"
        >
          {saving ? "Creating…" : "Create"}
        </button>
        <button
          onClick={() => create(true)}
          disabled={saving || parserId === null}
          className="touch-target-small"
          style={{ padding: "5px 14px", fontSize: 13 }}
          title="Creates the note and starts another with the same parser and tags. Ctrl+Shift+Enter (Cmd+Shift+Enter on a Mac)"
        >
          Create and New
        </button>
        <button
          onClick={cancel}
          disabled={saving}
          className="touch-target-small"
          style={{ padding: "5px 14px", fontSize: 13 }}
        >
          Cancel
        </button>
        {error && <span className="error-text-small">{error}</span>}
      </div>
    </div>
  );
}
