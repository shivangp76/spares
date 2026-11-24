import { useEffect, useId, useRef, useState } from "react";
import { Link, Navigate, useNavigate } from "react-router-dom";
import {
  NetworkError,
  getCredentials,
  getSchedulerNames,
  getSignupPolicy,
  login,
  saveCredentials,
  schedulerToUse,
  type SignupPolicy,
} from "../api/client";
import AuthLayout from "../components/AuthLayout";
import { LOGIN_DOCS, docUrl } from "../help/pages";
import { hostOf, isSameHost, nextPath } from "../login";
import { errorMessage } from "../utils";

export default function LoginPage() {
  const navigate = useNavigate();
  // A built app is served by the server it talks to
  // The dev server sends the API to the host (see vite.config.ts)
  const [serverUrl, setServerUrl] = useState(window.location.origin);
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
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
  const hostLabel = thisHost ? "this host" : hostOf(serverUrl);

  const passwordId = useId();
  const [showPassword, setShowPassword] = useState(false);
  const hostDetails = useRef<HTMLDetailsElement>(null);
  // Set on the element at once, not through state, so the browser can focus an invalid field in it
  // in the same submit
  function openHostDetails() {
    if (hostDetails.current) hostDetails.current.open = true;
  }

  /** Logs in, then picks the scheduler with the host before saving the session. */
  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!serverUrl || !user || !password) return;
    const url = serverUrl.replace(/\/$/, "");
    setChecking(true);
    setError(null);
    try {
      const session = await login(url, user.trim(), password);
      const schedulers = await getSchedulerNames(url);
      saveCredentials({
        serverUrl: url,
        csrfToken: session.csrfToken,
        user: session.user,
        schedulerName: schedulerToUse(schedulers),
      });
      navigate(next, { replace: true });
    } catch (e) {
      if (e instanceof NetworkError) {
        setError(
          "The host could not be reached. Check its URL under Host. A device that keeps a copy of the collection works offline once logged in.",
        );
        openHostDetails();
      } else {
        setError(errorMessage(e));
      }
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
      <form
        onSubmit={handleSubmit}
        // An error is about what was typed before, so editing any field clears it
        onChange={() => setError(null)}
        className="login-form"
      >
        <label>
          Name
          <input
            type="text"
            value={user}
            onChange={(e) => setUser(e.target.value)}
            autoComplete="username"
            autoCapitalize="none"
            autoFocus
            required
          />
          <span className="field-hint">
            Your account on the host, or its email address.
          </span>
        </label>
        <div>
          <label htmlFor={passwordId}>Password</label>
          <div className="password-field">
            <input
              id={passwordId}
              type={showPassword ? "text" : "password"}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
              required
            />
            <button
              type="button"
              className="touch-target-small"
              aria-pressed={showPassword}
              aria-controls={passwordId}
              onClick={() => setShowPassword((shown) => !shown)}
            >
              {showPassword ? "Hide" : "Show"}
            </button>
          </div>
        </div>
        {/* Rarely changed, so closed until an error needs it */}
        <details ref={hostDetails} className="login-advanced">
          <summary>
            Host <span className="hint">({hostLabel})</span>
          </summary>
          <label>
            Server URL
            <input
              type="url"
              value={serverUrl}
              onChange={(e) => setServerUrl(e.target.value)}
              // A field in a closed section can't be focused to show what's wrong with it
              onInvalid={openHostDetails}
              placeholder="https://your-server.example.com"
              required
            />
            <span className="field-hint">
              The address of the spares host, which is this page's when it
              serves the page.
            </span>
          </label>
        </details>
        <button
          type="submit"
          disabled={checking}
          className="button-primary touch-target login-submit"
        >
          {checking ? "Logging in…" : "Log in"}
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
