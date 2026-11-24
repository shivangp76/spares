import { useEffect, useState } from "react";
import { Link, Navigate, useNavigate } from "react-router-dom";
import {
  NetworkError,
  getCredentials,
  getSchedulerNames,
  getSignupPolicy,
  login,
  saveCredentials,
  type SignupPolicy,
} from "../api/client";
import AuthLayout from "../components/AuthLayout";
import { LOGIN_DOCS, docUrl } from "../help/pages";
import { isSameHost, nextPath } from "../login";
import { errorMessage } from "../utils";

export default function LoginPage() {
  const navigate = useNavigate();
  // A built app is served by the server it talks to
  // The dev server sends the API to the host (see vite.config.ts)
  const [serverUrl, setServerUrl] = useState(window.location.origin);
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  // Empty for the server's default scheduler
  const [schedulerName, setSchedulerName] = useState("");
  const [checking, setChecking] = useState(false);
  const [policy, setPolicy] = useState<SignupPolicy | null>(null);
  const verified = new URLSearchParams(window.location.search).has("verified");
  const [error, setError] = useState<string | null>(null);
  // Where a page that needed logging in sent us from
  const next = nextPath(window.location.search);
  const [loggedIn] = useState(() => {
    try {
      getCredentials();
      return true;
    } catch {
      return false;
    }
  });

  // This page's host's: the signup and password-reset pages talk to it, not to the typed one
  useEffect(() => {
    getSignupPolicy(window.location.origin).then(setPolicy, () =>
      setPolicy(null),
    );
  }, []);
  const thisHost = isSameHost(serverUrl, window.location.origin);

  /** Logs in, then checks the scheduler with the host before saving the session. */
  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!serverUrl || !user || !password) return;
    const url = serverUrl.replace(/\/$/, "");
    setChecking(true);
    setError(null);
    try {
      const session = await login(url, user.trim(), password);
      // The host lists its default first
      const schedulers = await getSchedulerNames(url);
      const name = schedulerName.trim() || schedulers[0];
      if (schedulers.includes(name)) {
        saveCredentials({
          serverUrl: url,
          csrfToken: session.csrfToken,
          user: session.user,
          schedulerName: name,
        });
        navigate(next, { replace: true });
      } else {
        setError(
          `The host has no scheduler named “${name}”. Its schedulers are ${schedulers.join(", ")}.`,
        );
      }
    } catch (e) {
      setError(
        e instanceof NetworkError
          ? "The host could not be reached. Check its URL. A device that keeps a copy of the collection works offline once logged in."
          : errorMessage(e),
      );
    } finally {
      setChecking(false);
    }
  }

  if (loggedIn) return <Navigate to={next} replace />;

  return (
    <AuthLayout
      title="Spares"
      error={error}
      links={
        <>
          {thisHost && policy && policy.signups !== "closed" && (
            <Link to="/signup">Create an account</Link>
          )}
          {thisHost && policy?.email && (
            <Link to="/reset-password">Forgot your password?</Link>
          )}
          <Link to={docUrl(LOGIN_DOCS)}>{LOGIN_DOCS.label}</Link>
        </>
      }
    >
      <form onSubmit={handleSubmit} className="login-form">
        <label>
          Server URL
          <input
            type="url"
            value={serverUrl}
            onChange={(e) => setServerUrl(e.target.value)}
            placeholder="https://your-server.example.com"
            required
          />
          <span className="field-hint">
            The address of the spares host, which is this page's when it serves
            the page.
          </span>
        </label>
        <label>
          Name
          <input
            type="text"
            value={user}
            onChange={(e) => setUser(e.target.value)}
            autoComplete="username"
            autoCapitalize="none"
            required
          />
          <span className="field-hint">
            Your account on the host, or its email address.
          </span>
        </label>
        <label>
          Password
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
            required
          />
        </label>
        <label>
          Scheduler
          <input
            type="text"
            value={schedulerName}
            onChange={(e) => setSchedulerName(e.target.value)}
            placeholder="The server's default"
          />
          <span className="field-hint">
            What schedules the cards you rate. The server's are checked when you
            log in.
          </span>
        </label>
        <button
          type="submit"
          disabled={checking}
          className="touch-target login-submit"
        >
          {checking ? "Checking…" : "Login"}
        </button>
        {verified && !error && (
          <div role="status" style={{ fontSize: 13 }}>
            Your email address is verified. Log in.
          </div>
        )}
      </form>
    </AuthLayout>
  );
}
