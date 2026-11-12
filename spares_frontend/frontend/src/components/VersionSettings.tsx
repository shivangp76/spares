import { useEffect, useState } from "react";
import { sectionLabel } from "../utils";
import {
  checkVersion,
  isOutdated,
  PAGE_BUILD,
  reloadToUpdate,
  useServerVersion,
} from "../version";

function formatTime(time: string | null): string {
  return time ? new Date(time).toLocaleString() : "unknown";
}

/** The builds this page and the server that served it run, to tell an old page apart (see src/version.ts). */
export default function VersionSettings() {
  const version = useServerVersion();
  const [checked, setChecked] = useState(false);
  // Ask again on opening Settings, rather than show an answer from before a deploy
  useEffect(() => {
    void checkVersion().finally(() => setChecked(true));
  }, []);
  const outdated = isOutdated(version);
  const served = version?.frontend?.build;

  return (
    <section style={{ marginTop: 24 }}>
      <div style={sectionLabel}>Version</div>
      <dl
        style={{
          display: "grid",
          gridTemplateColumns: "auto 1fr",
          gap: "4px 12px",
          margin: 0,
          fontSize: 13,
        }}
      >
        <dt>This page</dt>
        <dd style={{ margin: 0 }}>
          <code>{PAGE_BUILD ?? "development"}</code>
        </dd>
        <dt>Server&apos;s app</dt>
        <dd style={{ margin: 0 }}>
          {version ? (
            <code>{served ?? "none"}</code>
          ) : checked ? (
            "The server cannot be reached"
          ) : (
            "Asking the server…"
          )}
        </dd>
        {version && (
          <>
            <dt>Server</dt>
            <dd style={{ margin: 0 }}>
              {version.server.version}, built {formatTime(version.server.built)}
              , running since {formatTime(version.server.started)}
            </dd>
          </>
        )}
      </dl>
      {outdated && (
        <p style={{ fontSize: 13, color: "var(--warning)", margin: "8px 0 0" }}>
          This page is an older build than the server&apos;s.{" "}
          <button
            className="navbar-button"
            onClick={() => void reloadToUpdate()}
          >
            Reload
          </button>
        </p>
      )}
    </section>
  );
}
