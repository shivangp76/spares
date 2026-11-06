import { useEffect, useState } from "react";
import {
  listDeviceTokens,
  revokeDeviceToken,
  type DeviceToken,
} from "../api/client";
import { errorMessage, td, th } from "../utils";

const formatTime = (at: number | null) =>
  at === null ? "never" : new Date(at * 1000).toLocaleString();

/**
 * The account's device tokens: each `spares login`, and each host syncing with this one. Revoking
 * one logs that device out, as `spares logout` there would, e.g. for a lost laptop. This browser
 * uses a session rather than a token, so it is not listed.
 */
export default function DeviceTokens() {
  const [devices, setDevices] = useState<DeviceToken[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    listDeviceTokens()
      .then((d) => {
        if (!cancelled) setDevices(d);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function revoke(device: DeviceToken) {
    const copy =
      device.replica === null
        ? ""
        : ` Its copy of the collection (replica ${device.replica}) can no longer sync, so changes it has not synced are lost.`;
    if (
      !window.confirm(
        `Log out "${device.name}"? It needs \`spares login\` to reach this host again.${copy}`,
      )
    )
      return;
    setBusy(device.id);
    setError(null);
    setMessage(null);
    try {
      await revokeDeviceToken(device.id);
      setDevices((prev) => prev?.filter((d) => d.id !== device.id) ?? null);
      setMessage(`Logged out "${device.name}".`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ fontSize: 14, fontWeight: 600 }}>Logged-in devices</div>
      {devices && devices.length === 0 && (
        <p
          style={{ fontSize: 13, color: "var(--text-muted)", margin: "4px 0" }}
        >
          No device is logged in with <code>spares login</code>.
        </p>
      )}
      {devices && devices.length > 0 && (
        <div style={{ overflowX: "auto", marginTop: 4 }}>
          <table style={{ borderCollapse: "collapse", fontSize: 14 }}>
            <thead>
              <tr>
                <th style={th}>Name</th>
                <th style={th}>Access</th>
                <th style={th}>Logged in</th>
                <th style={th}>Last used</th>
                <th style={th}></th>
              </tr>
            </thead>
            <tbody>
              {devices.map((device) => (
                <tr key={device.id}>
                  <td style={td}>{device.name}</td>
                  <td style={td}>
                    {device.scope === "sync" ? "Sync only" : "Everything"}
                    {device.replica === null
                      ? ""
                      : `, keeps a copy (replica ${device.replica})`}
                  </td>
                  <td style={td}>{formatTime(device.created_at)}</td>
                  <td style={td}>{formatTime(device.last_used_at)}</td>
                  <td style={td}>
                    <button
                      disabled={busy !== null}
                      onClick={() => void revoke(device)}
                      className="error-text"
                    >
                      {busy === device.id ? "Logging out…" : "Log out"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {message && (
        <p
          role="status"
          style={{ marginTop: 8, color: "var(--text-secondary)" }}
        >
          {message}
        </p>
      )}
      {error && (
        <p role="alert" style={{ marginTop: 8, color: "var(--error)" }}>
          {error}
        </p>
      )}
    </div>
  );
}
