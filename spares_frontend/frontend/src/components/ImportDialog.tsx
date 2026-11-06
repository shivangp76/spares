import { useEffect, useRef, useState } from "react";
import { getKnownParserNames, importFiles, importShared } from "../api/client";
import { runOnServer } from "../local/copy";
import type {
  ImportRequestOptions,
  ImportResponse,
  ImportSharedResponse,
  RewrittenFile,
} from "../types/spares";
import { downloadBlob, errorMessage, tagList, td, th } from "../utils";
import type { ActionOutcome } from "./ActionResult";

/**
 * - `shared`: notes someone shared, added as new notes (`spares_core::api::note::share_bundle`)
 * - `collection`: files of this collection's notes, which may update and delete notes by id and
 *   hold live notes, as `spares import` imports them (`spares_core::api::note::import`)
 */
type Mode = "shared" | "collection";

/** What importing shared notes did, in a sentence or a few. */
function sharedSummary(imported: ImportSharedResponse, name: string): string {
  const parts = [
    `Imported ${imported.note_count} note(s) from ${name}, with ${imported.media_count} image, audio and image occlusion file(s).`,
  ];
  if (imported.parsers_added.length > 0)
    parts.push(`Added the parser(s) ${tagList(imported.parsers_added)}.`);
  if (imported.missing_media.length > 0)
    parts.push(
      `The zip has none of ${imported.missing_media.join(", ")}, which the notes name.`,
    );
  if (imported.skipped_files.length > 0)
    parts.push(
      `Left out ${imported.skipped_files.join(", ")}, which no parser reads.`,
    );
  return parts.join(" ");
}

/** How many notes an import adds, updates and deletes, and their cards. */
function counts(imported: ImportResponse) {
  const of = (action: string) =>
    imported.notes.filter((n) => n.action === action).length;
  return {
    added: of("add"),
    updated: of("update"),
    deleted: of("delete") + imported.deleted_live_note_ids.length,
    cards: imported.notes.reduce((sum, n) => sum + n.card_count, 0),
  };
}

/** What a collection import would do. */
function previewSummary(imported: ImportResponse): string {
  const { added, updated, deleted, cards } = counts(imported);
  return `Would add ${added}, update ${updated} and delete ${deleted} note(s), with ${cards} card(s).`;
}

/** What a collection import did, in a sentence or a few. */
function collectionSummary(imported: ImportResponse): string {
  const { added, updated, deleted, cards } = counts(imported);
  const parts = [
    `Imported: added ${added}, updated ${updated} and deleted ${deleted} note(s), with ${cards} card(s).`,
  ];
  if (imported.media_count > 0)
    parts.push(
      `Stored ${imported.media_count} image, audio and image occlusion file(s).`,
    );
  if (imported.rewritten_files.length > 0)
    parts.push("Download the rewritten file(s) to replace yours.");
  return parts.join(" ");
}

/** Problems that don't stop an import, for the preview and the result. */
function Notices({ imported }: { imported: ImportResponse }) {
  const items: string[] = [];
  if (imported.skipped_note_count > 0)
    items.push(
      `${imported.skipped_note_count} note(s) have no parseable data and are skipped.`,
    );
  if (imported.deleted_live_note_ids.length > 0)
    items.push(
      `Live note(s) ${imported.deleted_live_note_ids.join(", ")} are past the end of their sync group, so they are deleted.`,
    );
  if (imported.missing_media.length > 0)
    items.push(
      `A zip has none of ${imported.missing_media.join(", ")}, which its notes name.`,
    );
  if (imported.skipped_files.length > 0)
    items.push(
      `Left out ${imported.skipped_files.join(", ")}, which no parser reads.`,
    );
  if (items.length === 0) return null;
  return (
    <ul style={{ fontSize: 13, margin: "8px 0 0", paddingLeft: 20 }}>
      {items.map((item) => (
        <li key={item}>{item}</li>
      ))}
    </ul>
  );
}

/** The notes of an import, one row each. */
function NotesTable({ imported }: { imported: ImportResponse }) {
  const many = new Set(imported.notes.map((n) => n.file)).size > 1;
  return (
    <div style={{ overflow: "auto", maxHeight: 280, marginTop: 8 }}>
      <table
        style={{ borderCollapse: "collapse", fontSize: 13, width: "100%" }}
      >
        <thead>
          <tr>
            {many && <th style={th}>File</th>}
            <th style={th}>Action</th>
            <th style={th}>Cards</th>
            <th style={th}>Tags</th>
            <th style={th}>Note</th>
          </tr>
        </thead>
        <tbody>
          {imported.notes.map((note, i) => (
            <tr key={i}>
              {many && <td style={td}>{note.file}</td>}
              <td style={td}>
                {note.action}
                {note.note_id !== null ? ` ${note.note_id}` : ""}
                {note.live ? " (live)" : ""}
                {note.is_suspended ? ", suspended" : ""}
              </td>
              <td style={td}>{note.card_count}</td>
              <td style={td}>{tagList(note.tags)}</td>
              <td
                style={{
                  ...td,
                  maxWidth: 280,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
                title={note.data}
              >
                {note.data.trim().split("\n")[0]}
                {note.warnings.length > 0 && (
                  <div
                    style={{ color: "var(--warning)", whiteSpace: "normal" }}
                  >
                    {note.warnings.join("; ")}
                  </div>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Downloads a file an import rewrote, under its own name. */
function download(file: RewrittenFile) {
  const name = file.name.split("/").pop() || "notes";
  downloadBlob(name, new Blob([file.contents], { type: "text/plain" }));
}

/**
 * Imports files of notes: shared notes as new notes, or this collection's, with `spares import`'s
 * options and a preview of what it would do first.
 */
export default function ImportDialog({
  onImported,
  onClose,
}: {
  /** After an import changed the notes */
  onImported: (outcome: ActionOutcome) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [files, setFiles] = useState<File[]>([]);
  const [mode, setMode] = useState<Mode>("collection");
  const [parser, setParser] = useState("");
  const [toParser, setToParser] = useState("");
  const [stripLiveness, setStripLiveness] = useState(false);
  const [parsers, setParsers] = useState<string[]>([]);
  const [preview, setPreview] = useState<ImportResponse | null>(null);
  const [done, setDone] = useState<ImportResponse | null>(null);
  const [busy, setBusy] = useState<"preview" | "import" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  useEffect(() => {
    getKnownParserNames().then(setParsers, () => {});
  }, []);

  const options = (dryRun: boolean): ImportRequestOptions => ({
    parser: parser || null,
    to_parser: toParser || null,
    dry_run: dryRun,
    strip_liveness: stripLiveness,
  });

  /** Changes what to import, which the last preview no longer shows. */
  function change(update: () => void) {
    update();
    setPreview(null);
    setDone(null);
    setError(null);
  }

  function pickFiles(picked: File[]) {
    change(() => {
      setFiles(picked);
      // A zip is usually notes someone shared
      if (picked.length > 0)
        setMode(
          picked.every((f) => f.name.toLowerCase().endsWith(".zip"))
            ? "shared"
            : "collection",
        );
    });
  }

  async function run(kind: "preview" | "import", action: () => Promise<void>) {
    setBusy(kind);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const runPreview = () =>
    run("preview", async () => {
      setPreview(await importFiles(files, options(true)));
    });

  const runImport = () =>
    run("import", async () => {
      if (mode === "shared") {
        const messages: string[] = [];
        const eventIds: number[] = [];
        // One at a time, as the server takes them; each adds its own notes
        const { local, syncError } = await runOnServer(async () => {
          for (const file of files) {
            const result = await importShared(file);
            messages.push(sharedSummary(result, file.name));
            eventIds.push(...result.event_ids);
          }
        });
        onImported(outcome(messages.join(" "), eventIds, local, syncError));
        dialogRef.current?.close();
        return;
      }
      const { result, local, syncError } = await runOnServer(() =>
        importFiles(files, options(false)),
      );
      setDone(result);
      setPreview(null);
      onImported(
        outcome(
          collectionSummary(result),
          result.event_id === null ? [] : [result.event_id],
          local,
          syncError,
        ),
      );
      // Kept open to download the rewritten files
      if (result.rewritten_files.length === 0) dialogRef.current?.close();
    });

  const shown = done ?? preview;
  return (
    <dialog
      ref={dialogRef}
      className="audio-recorder import-dialog"
      aria-labelledby="import-dialog-title"
      onClose={onClose}
      onCancel={(e) => {
        if (busy) e.preventDefault();
      }}
    >
      <div className="audio-recorder-content">
        <h3 id="import-dialog-title" style={{ margin: "0 0 12px" }}>
          Import notes
        </h3>
        <input
          type="file"
          multiple
          accept=".zip,.md,.typ,.tex"
          aria-label="Files to import"
          disabled={busy !== null}
          onChange={(e) => pickFiles([...(e.target.files ?? [])])}
        />
        <fieldset
          style={{
            border: "none",
            padding: 0,
            margin: "12px 0 0",
            fontSize: 14,
          }}
          disabled={busy !== null}
        >
          <label style={{ display: "flex", gap: 6, alignItems: "baseline" }}>
            <input
              type="radio"
              name="import-mode"
              checked={mode === "collection"}
              onChange={() => change(() => setMode("collection"))}
            />
            <span>
              Import into this collection
              <span style={hint}>
                Adds, updates and deletes notes as the files say, as{" "}
                <code>spares import</code> does: an export to edit notes, or
                live notes.
              </span>
            </span>
          </label>
          <label
            style={{
              display: "flex",
              gap: 6,
              alignItems: "baseline",
              marginTop: 6,
            }}
          >
            <input
              type="radio"
              name="import-mode"
              checked={mode === "shared"}
              onChange={() => change(() => setMode("shared"))}
            />
            <span>
              Add as new notes
              <span style={hint}>
                Shared notes, from a zip exported to share or a file of notes,
                with the parsers they need.
              </span>
            </span>
          </label>
        </fieldset>
        {mode === "collection" && (
          <fieldset
            style={{
              border: "none",
              padding: 0,
              margin: "12px 0 0",
              display: "grid",
              gap: 8,
              fontSize: 14,
            }}
            disabled={busy !== null}
          >
            <label>
              Parser{" "}
              <select
                value={parser}
                onChange={(e) => change(() => setParser(e.target.value))}
              >
                <option value="">Guess from each file</option>
                {parsers.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Convert to{" "}
              <select
                value={toParser}
                onChange={(e) => change(() => setToParser(e.target.value))}
              >
                <option value="">Keep the files' parser</option>
                {parsers.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <label style={{ display: "flex", gap: 6, alignItems: "baseline" }}>
              <input
                type="checkbox"
                checked={stripLiveness}
                onChange={(e) =>
                  change(() => setStripLiveness(e.target.checked))
                }
              />
              <span>
                Strip liveness
                <span style={hint}>
                  Turns the files' live notes, already imported, into ordinary
                  notes, once they are stable.
                </span>
              </span>
            </label>
          </fieldset>
        )}
        {shown && (
          <div style={{ marginTop: 12 }}>
            <div style={{ fontSize: 14 }}>
              {done ? collectionSummary(done) : previewSummary(shown)}
            </div>
            <Notices imported={shown} />
            {!done && <NotesTable imported={shown} />}
            {shown.rewritten_files.length > 0 && (
              <div style={{ marginTop: 8, fontSize: 13 }}>
                {done
                  ? "The import rewrote these files (their clozes' id: keys). Replace yours with them before editing further:"
                  : "The import will rewrite these files, and offer them to download in place of yours:"}
                {done && (
                  <div
                    style={{
                      display: "flex",
                      gap: 8,
                      flexWrap: "wrap",
                      marginTop: 6,
                    }}
                  >
                    {done.rewritten_files.map((file) => (
                      <button
                        key={file.file}
                        onClick={() => download(file)}
                        className="touch-target-small"
                      >
                        Download {file.name.split("/").pop()}
                      </button>
                    ))}
                  </div>
                )}
                {!done && (
                  <ul style={{ margin: "4px 0 0", paddingLeft: 20 }}>
                    {shown.rewritten_files.map((file) => (
                      <li key={file.file}>{file.name}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </div>
        )}
        {error && (
          <p
            role="alert"
            style={{
              fontSize: 13,
              color: "var(--error)",
              margin: "12px 0 0",
              whiteSpace: "pre-wrap",
            }}
          >
            {error}
          </p>
        )}
        <div
          style={{
            display: "flex",
            justifyContent: "flex-end",
            gap: 8,
            marginTop: 16,
          }}
        >
          <button
            onClick={() => dialogRef.current?.close()}
            disabled={busy !== null}
            className="touch-target-small"
          >
            {done ? "Close" : "Cancel"}
          </button>
          {mode === "collection" && !done && (
            <button
              onClick={() => void runPreview()}
              disabled={busy !== null || files.length === 0}
              className="touch-target-small"
            >
              {busy === "preview" ? "Checking…" : "Preview"}
            </button>
          )}
          {!done && (
            <button
              onClick={() => void runImport()}
              disabled={
                busy !== null ||
                files.length === 0 ||
                (mode === "collection" && !preview)
              }
              title={
                mode === "collection" && !preview
                  ? "Preview first, to see what the import does"
                  : undefined
              }
              className="touch-target-small"
            >
              {busy === "import" ? "Importing…" : "Import"}
            </button>
          )}
        </div>
      </div>
    </dialog>
  );
}

const hint: React.CSSProperties = {
  display: "block",
  fontSize: 12,
  color: "var(--text-muted)",
};

/** What an import did, with Undo for its events, which a device's copy can't undo. */
function outcome(
  message: string,
  eventIds: number[],
  local: boolean,
  syncError: string | null,
): ActionOutcome {
  if (!local) return { message, eventIds };
  // The server added them, so this device's copy has them once it syncs. The server's events
  // aren't synced, so the copy can't undo them.
  return {
    message: syncError
      ? `${message} They reach this device at its next sync (${syncError}).`
      : message,
    eventIds: [],
  };
}
