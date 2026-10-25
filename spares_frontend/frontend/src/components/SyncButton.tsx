import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
  getCredentials,
  getSyncStatus,
  runSync,
  type SyncReport,
  type SyncStatus,
} from "../api/client";
import { syncLocalCopy, useLocalStatus } from "../local/copy";
import { usesLocalCopy } from "../local/transport";
import { errorMessage, formatNoteId } from "../utils";
import HelpDialog from "./HelpDialog";

/** How long ago `unixSeconds` was, e.g. `5 min ago`. */
function ago(unixSeconds: number): string {
  const minutes = Math.round((Date.now() / 1000 - unixSeconds) / 60);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} days ago`;
}

/** Shows `message` under the navbar, with a button to reload the page when `changed`. */
type OnStatus = (message: string, changed?: boolean) => void;

/** Whether a sync changed what pages show, which they show once reloaded. */
function changedHere(report: SyncReport): boolean {
  return report.mode === "full_download" || report.pulled > 0;
}

/** What a sync did, e.g. `Synced: received 3 changes, sent 1.` */
function describeReport(report: SyncReport): string {
  const conflicts =
    report.conflicts.length > 0
      ? ` ${report.conflicts.length} conflicts (${report.conflicts.map(formatNoteId).join(", ")}): resolve them on the Conflicts page.`
      : "";
  if (report.mode === "full_download")
    return `Downloaded the server's collection.${conflicts}`;
  if (report.mode === "full_upload")
    return `Uploaded this device's collection. Other devices download it at their next sync.${conflicts}`;
  return report.mode === null
    ? `Already in sync.${conflicts}`
    : `Synced: received ${report.pulled} changes, sent ${report.pushed}.${conflicts}`;
}

function usesCopy(): boolean {
  try {
    return usesLocalCopy(getCredentials().serverUrl);
  } catch {
    return false;
  }
}

/**
 * Syncs this device's copy of the collection with the server, or, on the laptop, the laptop's
 * server with the sync server (`spares sync cloud`), with the time of the last sync and the number
 * of unresolved conflicts. Shown only where there is something to sync with.
 */
export default function SyncButton({ onStatus }: { onStatus: OnStatus }) {
  // Rendered again when the copy starts or stops, e.g. right after Keep a Copy
  useLocalStatus();
  return usesCopy() ? (
    <LocalSyncButton onStatus={onStatus} />
  ) : (
    <ServerSyncButton onStatus={onStatus} />
  );
}

function ConflictsBadge({ count }: { count: number }) {
  if (count === 0) return null;
  return (
    <Link
      to="/conflicts"
      className="sync-conflict-badge"
      style={{ marginLeft: 4 }}
      title={`${count} unresolved conflicts`}
    >
      {count}
    </Link>
  );
}

function ServerSyncButton({ onStatus }: { onStatus: OnStatus }) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [syncing, setSyncing] = useState(false);

  useEffect(() => {
    // Older servers and offline clients have no status; the button just stays hidden
    getSyncStatus().then(setStatus, () => setStatus(null));
  }, []);

  if (!status || status.role !== "client" || !status.sync_url) return null;

  async function sync() {
    setSyncing(true);
    try {
      const report = await runSync();
      onStatus(describeReport(report), changedHere(report));
    } catch (e) {
      const message = errorMessage(e);
      onStatus(
        message.includes("full sync")
          ? `${message} Run \`spares sync cloud\` to choose which side to keep.`
          : message,
      );
    } finally {
      setSyncing(false);
      getSyncStatus().then(setStatus, () => {});
    }
  }

  const title = [
    status.last_sync_at
      ? `Last synced ${ago(status.last_sync_at)}`
      : "Never synced",
    status.conflicts > 0 ? `${status.conflicts} unresolved conflicts` : null,
    status.needs_full_sync
      ? `Needs a full sync: ${status.needs_full_sync}`
      : null,
  ]
    .filter(Boolean)
    .join(". ");

  return (
    <span style={{ display: "inline-flex", alignItems: "center" }}>
      <button
        onClick={sync}
        disabled={syncing}
        className="navbar-button"
        title={title}
      >
        {syncing ? "Syncing…" : "Sync"}
        {status.last_sync_at && !syncing && (
          <span
            style={{ fontSize: 11, color: "var(--text-muted)", marginLeft: 4 }}
          >
            {ago(status.last_sync_at)}
          </span>
        )}
      </button>
      <ConflictsBadge count={status.conflicts} />
    </span>
  );
}

function LocalSyncButton({ onStatus }: { onStatus: OnStatus }) {
  const local = useLocalStatus();
  // The choice of which collection to keep, closed for this page once picked or dismissed
  const [dismissed, setDismissed] = useState(false);

  async function sync(mode: "normal" | "full_download" | "full_upload") {
    try {
      const report = await syncLocalCopy(mode);
      onStatus(describeReport(report), changedHere(report));
    } catch (e) {
      onStatus(errorMessage(e));
    }
    setDismissed(false);
  }

  const title = [
    local.lastSyncAt ? `Last synced ${ago(local.lastSyncAt)}` : "Never synced",
    local.dirty > 0 ? `${local.dirty} changes to send` : null,
    local.conflicts > 0 ? `${local.conflicts} unresolved conflicts` : null,
    // The copy's sync errors already start with "Sync failed"
    local.error
      ? `Last sync failed: ${local.error.replace(/^Sync failed: /, "")}`
      : null,
  ]
    .filter(Boolean)
    .join(". ");

  return (
    <span style={{ display: "inline-flex", alignItems: "center" }}>
      <button
        onClick={() => sync("normal")}
        disabled={local.syncing}
        className="navbar-button"
        title={title}
      >
        {local.syncing ? "Syncing…" : "Sync"}
        {local.lastSyncAt && !local.syncing && (
          <span
            style={{ fontSize: 11, color: "var(--text-muted)", marginLeft: 4 }}
          >
            {ago(local.lastSyncAt)}
          </span>
        )}
      </button>
      <ConflictsBadge count={local.conflicts} />
      {local.fullSyncRequired && !local.syncing && !dismissed && (
        <HelpDialog
          title="Choose which collection to keep"
          intro="This device's copy and the server's collection can't be merged, for example because another device uploaded its whole collection. One has to replace the other."
          onClose={() => setDismissed(true)}
        >
          {(close) => (
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 12,
                marginTop: 12,
              }}
            >
              <button
                onClick={() => {
                  close();
                  sync("full_download");
                }}
              >
                Download the server's collection
                {local.dirty > 0
                  ? ` (discards ${local.dirty} changes made here)`
                  : ""}
              </button>
              <button
                onClick={() => {
                  close();
                  sync("full_upload");
                }}
              >
                Upload this device's collection (other devices will need to
                download it)
              </button>
            </div>
          )}
        </HelpDialog>
      )}
    </span>
  );
}
