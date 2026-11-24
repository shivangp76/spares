import { useState } from "react";
import { Link } from "react-router-dom";
import { requestPasswordReset, resetPassword } from "../api/client";
import AuthLayout from "../components/AuthLayout";
import { errorMessage } from "../utils";

/** A forgotten password: asks for a link by email, and, from the link, sets a new password. */
export default function ResetPasswordPage() {
  const token = new URLSearchParams(window.location.search).get("token");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (token) {
        await resetPassword(token, password);
        setMessage("Your password is changed. Log in with it.");
      } else {
        setMessage(await requestPasswordReset(email.trim()));
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <AuthLayout
      title={token ? "Choose a new password" : "Forgot your password?"}
      error={error}
      links={<Link to="/login">Log in</Link>}
    >
      {message ? (
        <p role="status">{message}</p>
      ) : (
        <form onSubmit={handleSubmit} className="login-form">
          {token ? (
            <label>
              New password
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="new-password"
                minLength={8}
                required
              />
            </label>
          ) : (
            <label>
              Your account&apos;s email address
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                autoComplete="email"
                required
              />
            </label>
          )}
          <button
            type="submit"
            disabled={busy}
            className="button-primary touch-target login-submit"
          >
            {token ? "Set password" : "Email me a link"}
          </button>
        </form>
      )}
    </AuthLayout>
  );
}
