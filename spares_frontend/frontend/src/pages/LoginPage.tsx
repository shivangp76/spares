import { useEffect, useId, useState } from "react";
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
import { nextPath } from "../login";
import { errorMessage } from "../utils";

export default function LoginPage() {
  const navigate = useNavigate();
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

  useEffect(() => {
    getSignupPolicy().then(setPolicy, () => setPolicy(null));
  }, []);

  const passwordId = useId();
  const [showPassword, setShowPassword] = useState(false);

  /** Logs in, then picks the scheduler with the host before saving the session. */
  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!user || !password) return;
    setChecking(true);
    setError(null);
    try {
      const session = await login(user.trim(), password);
      const schedulers = await getSchedulerNames();
      saveCredentials({
        csrfToken: session.csrfToken,
        user: session.user,
        accountId: session.accountId,
        schedulerName: schedulerToUse(schedulers),
      });
      navigate(next, { replace: true });
    } catch (e) {
      if (e instanceof NetworkError) {
        setError(
          "The host could not be reached. A device that keeps a copy of the collection works offline once logged in.",
        );
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
      intro="A spaced repetition flashcard app. Write notes in Markdown, LaTeX or Typst, and Spares turns them into cards and schedules their reviews."
      error={error}
      links={
        <>
          {policy && policy.signups !== "closed" && (
            <Link to="/signup">Create an account</Link>
          )}
          {policy?.email && (
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
