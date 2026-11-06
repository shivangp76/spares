import { useState } from 'react';
import { Link } from 'react-router-dom';
import { requestPasswordReset, resetPassword } from '../api/client';
import { errorMessage } from '../utils';

const field: React.CSSProperties = { display: 'block', width: '100%', marginTop: 4, padding: '6px 8px', boxSizing: 'border-box' };

/** A forgotten password: asks for a link by email, and, from the link, sets a new password. */
export default function ResetPasswordPage() {
  const serverUrl = window.location.origin;
  const token = new URLSearchParams(window.location.search).get('token');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (token) {
        await resetPassword(serverUrl, token, password);
        setMessage('Your password is changed. Log in with it.');
      } else {
        setMessage(await requestPasswordReset(serverUrl, email.trim()));
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login" style={{ maxWidth: 400, margin: '80px auto', padding: 24 }}>
      <h2 style={{ marginBottom: 24 }}>{token ? 'Choose a new password' : 'Forgot your password?'}</h2>
      {message ? <p role="status">{message}</p> : (
        <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          {token ? (
            <label>
              New password
              <input type="password" value={password} onChange={e => setPassword(e.target.value)} autoComplete="new-password" minLength={8} required style={field} />
            </label>
          ) : (
            <label>
              Your account&apos;s email address
              <input type="email" value={email} onChange={e => setEmail(e.target.value)} autoComplete="email" required style={field} />
            </label>
          )}
          <button type="submit" disabled={busy} className="touch-target" style={{ padding: '8px 0' }}>
            {token ? 'Set password' : 'Email me a link'}
          </button>
        </form>
      )}
      {error && <div role="alert" style={{ color: 'var(--error)', fontSize: 13, marginTop: 12 }}>{error}</div>}
      <p style={{ fontSize: 13, marginTop: 16 }}><Link to="/login">Log in</Link></p>
    </div>
  );
}
