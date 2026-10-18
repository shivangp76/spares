import { useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { NetworkError, getCredentials, getSchedulerNames, saveCredentials } from '../api/client';
import { LOGIN_DOCS, docUrl } from '../help/pages';
import { errorMessage } from '../utils';

const hint: React.CSSProperties = { display: 'block', fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 };

export default function LoginPage() {
  const navigate = useNavigate();
  // A built app is served by the server it talks to
  const [serverUrl, setServerUrl] = useState(import.meta.env.PROD ? window.location.origin : '');
  const [apiKey, setApiKey] = useState('');
  const [schedulerName, setSchedulerName] = useState('fsrs');
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Offline, the server can't check them, so they can be saved as they are
  const [unreachable, setUnreachable] = useState(false);

  useEffect(() => {
    try {
      getCredentials();
      navigate('/review');
    } catch {
      // not logged in
    }
  }, [navigate]);

  function save() {
    saveCredentials({ serverUrl: serverUrl.replace(/\/$/, ''), apiKey, schedulerName });
    navigate('/review');
  }

  /** Checks the key and the scheduler with the server before saving them. */
  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!serverUrl || !apiKey) return;
    setChecking(true);
    setError(null);
    setUnreachable(false);
    try {
      const schedulers = await getSchedulerNames({ serverUrl: serverUrl.replace(/\/$/, ''), apiKey });
      if (schedulers.includes(schedulerName)) save();
      else setError(`The server has no scheduler named “${schedulerName}”. Its schedulers are ${schedulers.join(', ')}.`);
    } catch (e) {
      setUnreachable(e instanceof NetworkError);
      setError(e instanceof NetworkError ? 'The server could not be reached. Check its URL, or log in anyway to review offline.' : errorMessage(e));
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
          <span style={hint}>The address of <code>spares_server</code>, which is this page's when it serves the page.</span>
        </label>
        <label>
          API Key
          <input type="password" value={apiKey} onChange={e => setApiKey(e.target.value)}
            required style={field} />
          <span style={hint}>The server's <code>SPARES_API_KEY</code>.</span>
        </label>
        <label>
          Scheduler
          <input type="text" value={schedulerName} onChange={e => setSchedulerName(e.target.value)}
            style={field} />
          <span style={hint}>What schedules the cards you rate. The server's are checked when you log in.</span>
        </label>
        <button type="submit" disabled={checking} className="touch-target" style={{ padding: '8px 0', marginTop: 4 }}>
          {checking ? 'Checking…' : 'Login'}
        </button>
        {error && <div style={{ color: 'var(--error)', fontSize: 13 }}>{error}</div>}
        {unreachable && <button type="button" onClick={save} className="touch-target">Log in anyway</button>}
        <Link to={docUrl(LOGIN_DOCS)} style={{ fontSize: 13 }}>{LOGIN_DOCS.label}</Link>
      </form>
    </div>
  );
}
