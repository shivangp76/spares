import { useEffect, useState } from "react";
import { useDataVersion } from "../dataVersion";
import { useNavigate } from "react-router-dom";
import {
  describeReverted,
  listEvents,
  redoEvent,
  undoEvent,
} from "../api/client";
import { useAuth } from "../hooks/useAuth";
import type { EventEntry } from "../types/spares";
import { errorMessage, formatDateTime, td, th } from "../utils";
import PageTitle from "../components/PageTitle";

const PAGE_SIZE = 30;

/** What an entry is, e.g. "Undo of" before its description. */
function actionLabel(entry: EventEntry): string {
  if (entry.action === "Undo") return "Undid";
  if (entry.action === "Redo") return "Redid";
  return "";
}

/**
 * Every change to the collection, newest first, as `spares event undo <id>` and `spares event redo
 * <id>` reach them: any change can be undone (an action or a redo) or redone (an undo), not only
 * the latest, as long as nothing since depends on it. A group, such as an import, is one entry.
 */
export default function HistoryPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const [entries, setEntries] = useState<EventEntry[]>([]);
  const [ratings, setRatings] = useState(false);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  // Whether the first page came
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState<number | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Bumped to fetch the first page again, after an undo or redo
  const [version, setVersion] = useState(0);
  // And after the navbar's, or a sync
  const dataVersion = useDataVersion();

  useEffect(() => {
    if (!credentials) {
      navigate("/login");
      return;
    }
    let cancelled = false;
    listEvents(null, ratings, PAGE_SIZE)
      .then((page) => {
        if (cancelled) return;
        setEntries(page);
        setMore(page.length === PAGE_SIZE);
        setError(null);
        setLoaded(true);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [credentials, navigate, ratings, version, dataVersion]);

  /** The next page, older than the entries shown */
  async function loadOlder() {
    const before = entries[entries.length - 1]?.id ?? null;
    setLoading(true);
    try {
      const page = await listEvents(before, ratings, PAGE_SIZE);
      setEntries((prev) => [...prev, ...page]);
      setMore(page.length === PAGE_SIZE);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }

  async function revert(entry: EventEntry) {
    const redo = entry.action === "Undo";
    setBusy(entry.id);
    setStatus(null);
    setError(null);
    try {
      if (redo) {
        const res = await redoEvent(entry.id);
        setStatus(
          res
            ? describeReverted("Redone", res.redone_events)
            : "Nothing to redo.",
        );
      } else {
        const res = await undoEvent(entry.id);
        setStatus(
          res
            ? describeReverted("Undone", res.undone_events)
            : "Nothing to undo.",
        );
      }
      setVersion((v) => v + 1);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="page">
      <PageTitle
        description={
          <>
            Every change, newest first. Any of them can be undone, or an undo
            redone, unless a later change depends on it. A group, such as an
            import, is undone as one.
          </>
        }
      >
        History
      </PageTitle>
      <label
        style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 14 }}
      >
        <input
          type="checkbox"
          checked={ratings}
          onChange={(e) => setRatings(e.target.checked)}
        />
        Show ratings
      </label>
      {status && (
        <p role="status" className="secondary-text">
          {status}
        </p>
      )}
      {error && (
        <p
          role="alert"
          style={{
            fontSize: 13,
            color: "var(--error)",
            whiteSpace: "pre-wrap",
          }}
        >
          {error}
        </p>
      )}
      <div style={{ overflowX: "auto", marginTop: 12 }}>
        <table style={{ borderCollapse: "collapse", fontSize: 14 }}>
          <thead>
            <tr>
              <th style={th}>When</th>
              <th style={th}>Change</th>
              <th style={th}></th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry) => (
              <tr
                key={entry.id}
                style={
                  entry.reverted ? { color: "var(--text-muted)" } : undefined
                }
              >
                <td style={{ ...td, whiteSpace: "nowrap" }}>
                  {formatDateTime(entry.created_at)}
                </td>
                <td style={td}>
                  {actionLabel(entry) && (
                    <strong>{actionLabel(entry)}: </strong>
                  )}
                  {entry.description}
                  {entry.automatic && (
                    <span style={{ fontSize: 12, color: "var(--text-muted)" }}>
                      {" "}
                      · automatic
                    </span>
                  )}
                  {entry.reverted && (
                    <span style={{ fontSize: 12 }}>
                      {" "}
                      · {entry.action === "Undo" ? "redone" : "undone"}
                    </span>
                  )}
                </td>
                <td style={td}>
                  {!entry.reverted && (
                    <button
                      onClick={() => void revert(entry)}
                      disabled={busy !== null}
                      className="touch-target-small"
                    >
                      {busy === entry.id
                        ? "…"
                        : entry.action === "Undo"
                          ? "Redo"
                          : "Undo"}
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {loaded && entries.length === 0 && !error && (
        <p style={{ color: "var(--text-secondary)" }}>No changes yet.</p>
      )}
      {more && (
        <button
          onClick={() => void loadOlder()}
          disabled={loading}
          className="touch-target-small"
          style={{ marginTop: 12 }}
        >
          {loading ? "Loading…" : "Older"}
        </button>
      )}
    </div>
  );
}
