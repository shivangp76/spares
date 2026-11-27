import { useEffect, useState } from "react";
import { importTable } from "../api/client";
import {
  setTableImportParser,
  TABLE_IMPORT_PARSERS,
  useTableImportParser,
  type TableImportParser,
} from "../preferences";
import type {
  ImportTableRequest,
  ImportTableResponse,
  TableColumns,
  TableDelimiter,
} from "../types/spares";
import { errorMessage, tagList, td, th } from "../utils";

const PARSER_LABELS: Record<TableImportParser, string> = {
  markdown: "Markdown",
  typst: "Typst",
  "latex-note": "LaTeX",
};

const DELIMITER_LABELS: Record<TableDelimiter, string> = {
  tab: "Tab",
  comma: "Comma",
  semicolon: "Semicolon",
  dash: "Dash (term - definition)",
};

/** What the preview shows, for the table as it was when asked */
interface Preview {
  request: ImportTableRequest;
  response: ImportTableResponse;
}

/**
 * Imports a table of cards pasted from Google Sheets, Excel or Quizlet's export, or read from a
 * CSV or TSV file, as new notes: one per row, whose back is a cloze. Previews the first notes
 * before anything is written (`spares_core::adapters::impls::delimited`).
 */
export default function TableImport({
  busy,
  onImport,
  onCancel,
}: {
  busy: boolean;
  onCancel: () => void;
  /** Imports the table, resolving once it is imported */
  onImport: (request: ImportTableRequest) => Promise<void>;
}) {
  const [text, setText] = useState("");
  const [delimiter, setDelimiter] = useState<TableDelimiter | null>(null);
  const [hasHeader, setHasHeader] = useState<boolean | null>(null);
  const [columns, setColumns] = useState<TableColumns | null>(null);
  const parser = useTableImportParser();
  const [raw, setRaw] = useState(false);
  const [tag, setTag] = useState("");
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);

  const request = (dryRun: boolean): ImportTableRequest => ({
    text,
    delimiter,
    has_header: hasHeader,
    columns,
    parser,
    escape: !raw,
    tag: tag.trim() || null,
    dry_run: dryRun,
  });
  const current = request(true);
  const key = JSON.stringify(current);

  // A moment after the last change, so typing in the box doesn't ask for each key
  useEffect(() => {
    if (!text.trim()) return;
    let cancelled = false;
    const asked = JSON.parse(key) as ImportTableRequest;
    const timer = setTimeout(() => {
      importTable(asked).then(
        (response) => {
          if (cancelled) return;
          setPreview({ request: asked, response });
          setError(null);
        },
        (e) => {
          if (!cancelled) setError(errorMessage(e));
        },
      );
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [key, text]);

  const shown = text.trim() ? preview?.response : undefined;
  const upToDate = !!preview && JSON.stringify(preview.request) === key;

  /** Changes how the table is read, which picks its columns again */
  function reread(update: () => void) {
    update();
    setColumns(null);
  }

  async function load(file: File | undefined) {
    if (!file) return;
    try {
      reread(() => {
        setDelimiter(null);
        setHasHeader(null);
      });
      setText(await file.text());
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const columnCount = shown?.column_count ?? 0;
  const used = columns ?? shown?.columns;
  const columnName = (i: number) =>
    shown?.header?.[i]?.trim() || `Column ${i + 1}`;
  const columnSelect = (
    label: string,
    value: number | null,
    set: (column: number | null) => void,
    optional = false,
  ) => (
    <label>
      {label}{" "}
      <select
        value={value ?? ""}
        onChange={(e) =>
          set(e.target.value === "" ? null : Number(e.target.value))
        }
        disabled={columnCount === 0}
      >
        {optional && <option value="">None</option>}
        {Array.from({ length: columnCount }, (_, i) => (
          <option key={i} value={i}>
            {columnName(i)}
          </option>
        ))}
      </select>
    </label>
  );
  const setColumn = (field: keyof TableColumns) => (column: number | null) => {
    if (!used) return;
    setColumns({ ...used, [field]: field === "tags" ? column : (column ?? 0) });
  };

  return (
    <>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        aria-label="Table to import"
        placeholder={
          "Paste a table from Google Sheets, Excel or Quizlet's export:\nperro\tdog\ngato\tcat\n\nor lines like\nmitochondria - the powerhouse of the cell"
        }
        rows={8}
        spellCheck={false}
        disabled={busy}
        className="table-import-text"
      />
      <div style={{ marginTop: 6, fontSize: 13 }}>
        <label>
          Or read a CSV or TSV file:{" "}
          <input
            type="file"
            accept=".csv,.tsv,.txt,text/csv,text/tab-separated-values,text/plain"
            disabled={busy}
            onChange={(e) => void load(e.target.files?.[0])}
          />
        </label>
      </div>
      <fieldset className="table-import-options" disabled={busy}>
        <label>
          Separated by{" "}
          <select
            value={delimiter ?? ""}
            onChange={(e) =>
              reread(() =>
                setDelimiter(
                  e.target.value === ""
                    ? null
                    : (e.target.value as TableDelimiter),
                ),
              )
            }
          >
            <option value="">
              {shown && delimiter === null
                ? `Detected: ${DELIMITER_LABELS[shown.delimiter]}`
                : "Detect"}
            </option>
            {(Object.keys(DELIMITER_LABELS) as TableDelimiter[]).map((d) => (
              <option key={d} value={d}>
                {DELIMITER_LABELS[d]}
              </option>
            ))}
          </select>
        </label>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={hasHeader ?? !!shown?.header}
            onChange={(e) => reread(() => setHasHeader(e.target.checked))}
          />
          First row is a header
        </label>
        {columnSelect("Front", used?.front ?? null, setColumn("front"))}
        {columnSelect("Back", used?.back ?? null, setColumn("back"))}
        {columnSelect("Tags", used?.tags ?? null, setColumn("tags"), true)}
        <label>
          Markup{" "}
          <select
            value={parser}
            onChange={(e) =>
              setTableImportParser(e.target.value as TableImportParser)
            }
          >
            {TABLE_IMPORT_PARSERS.map((p) => (
              <option key={p} value={p}>
                {PARSER_LABELS[p]}
              </option>
            ))}
          </select>
        </label>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={raw}
            onChange={(e) => setRaw(e.target.checked)}
          />
          Cells are written in {PARSER_LABELS[parser]} already
        </label>
        <label>
          Tag every note{" "}
          <input
            type="text"
            value={tag}
            onChange={(e) => setTag(e.target.value)}
            placeholder="e.g. spanish"
            size={14}
          />
        </label>
      </fieldset>
      {shown && (
        <div style={{ marginTop: 12, opacity: upToDate ? 1 : 0.6 }}>
          <div style={{ fontSize: 14 }}>
            Would add {shown.note_count} note(s), a card each.
            {shown.skipped_records.length > 0 &&
              ` Leaves out row(s) ${shown.skipped_records.join(", ")}, with an empty front or back.`}
          </div>
          <div style={{ overflow: "auto", maxHeight: 240, marginTop: 8 }}>
            <table
              style={{
                borderCollapse: "collapse",
                fontSize: 13,
                width: "100%",
              }}
            >
              <thead>
                <tr>
                  <th style={th}>Row</th>
                  <th style={th}>Front</th>
                  <th style={th}>Back</th>
                  <th style={th}>Tags</th>
                </tr>
              </thead>
              <tbody>
                {shown.preview.map((note) => (
                  <tr key={note.record}>
                    <td style={td}>{note.record}</td>
                    <td
                      style={{ ...td, whiteSpace: "pre-wrap" }}
                      title={note.data}
                    >
                      {note.front}
                    </td>
                    <td style={{ ...td, whiteSpace: "pre-wrap" }}>
                      {note.back}
                    </td>
                    <td style={td}>{tagList(note.tags)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {shown.note_count > shown.preview.length && (
            <div
              style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 4 }}
            >
              and {shown.note_count - shown.preview.length} more
            </div>
          )}
        </div>
      )}
      {error && (
        <p
          role="alert"
          className="error-text"
          style={{ fontSize: 13, margin: "12px 0 0", whiteSpace: "pre-wrap" }}
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
          onClick={onCancel}
          disabled={busy}
          className="touch-target-small"
        >
          Cancel
        </button>
        <button
          onClick={() => void onImport(request(false))}
          disabled={busy || !upToDate || !shown || shown.note_count === 0}
          title={
            upToDate
              ? undefined
              : "Wait for the preview, to see what the import does"
          }
          className="touch-target-small"
        >
          {busy ? "Importing…" : `Import ${shown?.note_count ?? 0} note(s)`}
        </button>
      </div>
    </>
  );
}
