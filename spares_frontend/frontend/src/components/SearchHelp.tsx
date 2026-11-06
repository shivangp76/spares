import { useEffect, useState } from "react";
import { explainQuery, getSearchExamples } from "../api/client";
import type { QueryExample, SearchField } from "../types/spares";
import { errorMessage } from "../utils";
import CodeText from "./CodeText";
import HelpDialog from "./HelpDialog";

/** How the query in the box parses and the SQL it runs, as `spares note search --explain` prints them. */
function Explain({
  query,
  items,
}: {
  query: string;
  items: "Notes" | "Cards";
}) {
  const [explanation, setExplanation] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function explain() {
    setBusy(true);
    setError(null);
    try {
      setExplanation(await explainQuery(query, items));
    } catch (e) {
      setExplanation(null);
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (!query.trim()) return null;
  return (
    <div style={{ marginTop: 8 }}>
      <button
        type="button"
        onClick={() => void explain()}
        disabled={busy}
        title="How the query parses, and the SQL it runs"
      >
        {busy ? "Explaining…" : "Explain this query"}
      </button>{" "}
      <code>{query}</code>
      {error && (
        <div style={{ color: "var(--error)", whiteSpace: "pre-wrap" }}>
          {error}
        </div>
      )}
      {explanation && (
        <pre
          style={{
            whiteSpace: "pre-wrap",
            fontSize: 12,
            margin: "8px 0 0",
            padding: 8,
            background: "var(--surface)",
            borderRadius: 4,
            maxHeight: 240,
            overflow: "auto",
          }}
        >
          {explanation}
        </pre>
      )}
    </div>
  );
}

/**
 * A popup with example queries and the fields, either of which `onPick` puts in the search box,
 * and how the query in the box, `query`, parses for finding `items`.
 */
export default function SearchHelp({
  fields,
  query,
  items,
  onPick,
  onClose,
}: {
  fields: SearchField[];
  query: string;
  items: "Notes" | "Cards";
  onPick: (query: string) => void;
  onClose: () => void;
}) {
  const [examples, setExamples] = useState<QueryExample[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    getSearchExamples().then(setExamples, (e) => setError(errorMessage(e)));
  }, []);

  // The examples under their sections, in order, and then the fields
  const sections: [string, { query: string; about: string }[]][] = [];
  for (const example of examples) {
    const last = sections[sections.length - 1];
    if (last?.[0] === example.section) last[1].push(example);
    else sections.push([example.section, [example]]);
  }
  if (fields.length > 0)
    sections.push([
      "Fields",
      fields.map((f) => ({ query: f.example, about: f.about })),
    ]);

  return (
    <HelpDialog
      title="Search syntax"
      wide
      intro={
        <>
          Tab completes fields and tags. Click a query to put it in the search
          box.
          {error && <div style={{ color: "var(--error)" }}>{error}</div>}
          <Explain query={query} items={items} />
        </>
      }
      onClose={onClose}
    >
      {(close) => (
        // One table, so the examples and the fields line up
        <table className="search-help-table">
          <tbody>
            {sections.map(([section, rows], i) => [
              <tr key={section}>
                <th
                  colSpan={2}
                  style={{
                    textAlign: "left",
                    padding: i === 0 ? "0 8px 4px" : "16px 8px 4px",
                  }}
                >
                  {section}
                </th>
              </tr>,
              ...rows.map(({ query, about }) => (
                <tr key={`${section} ${query}`}>
                  <td>
                    <button
                      type="button"
                      onClick={() => {
                        close();
                        onPick(query);
                      }}
                      title="Put this in the search box"
                    >
                      <code>{query}</code>
                    </button>
                  </td>
                  <td>
                    <CodeText text={about} />
                  </td>
                </tr>
              )),
            ])}
          </tbody>
        </table>
      )}
    </HelpDialog>
  );
}
