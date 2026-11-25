import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import {
  getSchedulerNames,
  getSignupPolicy,
  saveCredentials,
  schedulerToUse,
  signUp,
  type SignupPolicy,
} from "../api/client";
import AuthLayout from "../components/AuthLayout";
import { errorMessage } from "../utils";

/** Creating an account, as the host's signup policy allows: by invite, or with an email address to verify. */
export default function SignupPage() {
  const navigate = useNavigate();
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
    getSignupPolicy().then(setPolicy, (e) => setError(errorMessage(e)));
  }, []);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const result = await signUp({
        name: name.trim(),
        email: email.trim(),
        password,
        invite: invite.trim(),
      });
      if ("verify" in result) {
        setMessage(result.message);
      } else {
        const schedulers = await getSchedulerNames();
        saveCredentials({
          csrfToken: result.csrfToken,
          user: result.user,
        accountId: result.accountId,
          schedulerName: schedulerToUse(schedulers),
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
    <AuthLayout
      title="Create an account"
      error={error}
      links={<Link to="/login">Log in instead</Link>}
    >
      {policy?.signups === "closed" && (
        <p>This host takes no new accounts. Its admin can add one.</p>
      )}
      {message && <p role="status">{message}</p>}
      {policy && policy.signups !== "closed" && !message && (
        <form onSubmit={handleSubmit} className="login-form">
          <label>
            Name
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="username"
              autoCapitalize="none"
              required
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
              />
            </label>
          )}
          <button
            type="submit"
            disabled={busy}
            className="button-primary touch-target login-submit"
          >
            {busy ? "Creating…" : "Create account"}
          </button>
        </form>
      )}
    </AuthLayout>
  );
}
