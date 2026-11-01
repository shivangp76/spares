import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import {
  getSchedulerNames,
  getSignupPolicy,
  saveCredentials,
  signUp,
  type SignupPolicy,
} from "../api/client";
import { errorMessage } from "../utils";

const field: React.CSSProperties = {
  display: "block",
  width: "100%",
  marginTop: 4,
  padding: "6px 8px",
  boxSizing: "border-box",
};

/** Creating an account, as the host's signup policy allows: by invite, or with an email address to verify. */
export default function SignupPage() {
  const navigate = useNavigate();
  const serverUrl = window.location.origin;
  const [policy, setPolicy] = useState<SignupPolicy | null>(null);
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState(
    new URLSearchParams(window.location.search).get("invite") ?? "",
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    getSignupPolicy(serverUrl).then(setPolicy, (e) =>
      setError(errorMessage(e)),
    );
  }, [serverUrl]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const result = await signUp(serverUrl, {
        name: name.trim(),
        email: email.trim(),
        password,
        invite: invite.trim(),
      });
      if ("verify" in result) {
        setMessage(result.message);
      } else {
        const schedulers = await getSchedulerNames(serverUrl);
        saveCredentials({
          serverUrl,
          csrfToken: result.csrfToken,
          user: result.user,
          schedulerName: schedulers[0],
        });
        navigate("/review");
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="login"
      style={{ maxWidth: 400, margin: "80px auto", padding: 24 }}
    >
      <h2 style={{ marginBottom: 24 }}>Create an account</h2>
      {policy?.signups === "closed" && (
        <p>This host takes no new accounts. Its admin can add one.</p>
      )}
      {message && <p role="status">{message}</p>}
      {policy && policy.signups !== "closed" && !message && (
        <form
          onSubmit={handleSubmit}
          style={{ display: "flex", flexDirection: "column", gap: 14 }}
        >
          <label>
            Name
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="username"
              autoCapitalize="none"
              required
              style={field}
            />
          </label>
          <label>
            Email
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              autoComplete="email"
              required={policy.signups === "open"}
              style={field}
            />
          </label>
          <label>
            Password
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="new-password"
              minLength={8}
              required
              style={field}
            />
          </label>
          {policy.signups === "invite" && (
            <label>
              Invite code
              <input
                type="text"
                value={invite}
                onChange={(e) => setInvite(e.target.value)}
                required
                style={field}
              />
            </label>
          )}
          <button
            type="submit"
            disabled={busy}
            className="touch-target"
            style={{ padding: "8px 0", marginTop: 4 }}
          >
            {busy ? "Creating…" : "Create account"}
          </button>
        </form>
      )}
      {error && (
        <div
          role="alert"
          style={{ color: "var(--error)", fontSize: 13, marginTop: 12 }}
        >
          {error}
        </div>
      )}
      <p style={{ fontSize: 13, marginTop: 16 }}>
        <Link to="/login">Log in instead</Link>
      </p>
    </div>
  );
}
