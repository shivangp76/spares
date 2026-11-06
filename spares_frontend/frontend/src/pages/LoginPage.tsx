import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { NetworkError, getCredentials, getSchedulerNames, getSignupPolicy, login, saveCredentials, type SignupPolicy } from '../api/client';
import { LOGIN_DOCS, docUrl } from '../help/pages';
import { errorMessage } from '../utils';

const hint: React.CSSProperties = { display: 'block', fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 };

export default function LoginPage() {
  const navigate = useNavigate();
  // A built app is served by the server it talks to
  // The dev server sends the API to the host (see vite.config.ts)
  const [serverUrl, setServerUrl] = useState(window.location.origin);
  const [user, setUser] = useState('');
  const [password, setPassword] = useState('');
  // Empty for the server's default scheduler
  const [schedulerName, setSchedulerName] = useState('');
  const [checking, setChecking] = useState(false);
  const [policy, setPolicy] = useState<SignupPolicy | null>(null);
  const verified = new URLSearchParams(window.location.search).has('verified');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    try {
      getCredentials();
      navigate('/review');
    } catch {
      // not logged in
    }
  }, [navigate]);

  useEffect(() => {
    getSignupPolicy(window.location.origin).then(setPolicy, () => setPolicy(null));
  }, []);

  /** Logs in, then checks the scheduler with the host before saving the session. */
  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!serverUrl || !user || !password) return;
    const url = serverUrl.replace(/\/$/, '');
    setChecking(true);
    setError(null);
    try {
      const session = await login(url, user.trim(), password);
      // The host lists its default first
      const schedulers = await getSchedulerNames(url);
      const name = schedulerName.trim() || schedulers[0];
      if (schedulers.includes(name)) {
        saveCredentials({ serverUrl: url, csrfToken: session.csrfToken, user: session.user, schedulerName: name });
        navigate('/review');
      } else {
        setError(`The host has no scheduler named “${name}”. Its schedulers are ${schedulers.join(', ')}.`);
      }
    } catch (e) {
      setError(e instanceof NetworkError ? 'The host could not be reached. Check its URL. A device that keeps a copy of the collection works offline once logged in.' : errorMessage(e));
    } finally {
      setChecking(false);
    }
  }

  const field: React.CSSProperties = { display: 'block', width: '100%', marginTop: 4, padding: '6px 8px', boxSizing: 'border-box' };

  return (
    <div className="login" style={{ maxWidth: 400, margin: '80px auto', padding: 24 }}>
      <h2 style={{ marginBottom: 24 }}>Spares</h2>
      <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
        <label>
          Server URL
          <input type="url" value={serverUrl} onChange={e => setServerUrl(e.target.value)}
            placeholder="https://your-server.example.com" required style={field} />
          <span style={hint}>The address of the spares host, which is this page's when it serves the page.</span>
        </label>
        <label>
          Name
          <input type="text" value={user} onChange={e => setUser(e.target.value)} autoComplete="username"
            autoCapitalize="none" required style={field} />
          <span style={hint}>Your account on the host, or its email address.</span>
        </label>
        <label>
          Password
          <input type="password" value={password} onChange={e => setPassword(e.target.value)}
            autoComplete="current-password" required style={field} />
        </label>
        <label>
          Scheduler
          <input type="text" value={schedulerName} onChange={e => setSchedulerName(e.target.value)}
            placeholder="The server's default" style={field} />
          <span style={hint}>What schedules the cards you rate. The server's are checked when you log in.</span>
        </label>
        <button type="submit" disabled={checking} className="touch-target" style={{ padding: '8px 0', marginTop: 4 }}>
          {checking ? 'Checking…' : 'Login'}
        </button>
        {verified && !error && <div role="status" style={{ fontSize: 13 }}>Your email address is verified. Log in.</div>}
        {error && <div style={{ color: 'var(--error)', fontSize: 13 }}>{error}</div>}
        {policy && policy.signups !== 'closed' && <Link to="/signup" style={{ fontSize: 13 }}>Create an account</Link>}
        {policy?.email && <Link to="/reset-password" style={{ fontSize: 13 }}>Forgot your password?</Link>}
        <Link to={docUrl(LOGIN_DOCS)} style={{ fontSize: 13 }}>{LOGIN_DOCS.label}</Link>
      </form>
    </div>
  );
}
