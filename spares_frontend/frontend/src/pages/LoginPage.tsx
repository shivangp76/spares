import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { getCredentials, saveCredentials } from '../api/client';

export default function LoginPage() {
  const navigate = useNavigate();
  const [serverUrl, setServerUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [schedulerName, setSchedulerName] = useState('fsrs');

  useEffect(() => {
    try {
      getCredentials();
      navigate('/review');
    } catch {
      // not logged in
    }
  }, [navigate]);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!serverUrl || !apiKey) return;
    saveCredentials({ serverUrl: serverUrl.replace(/\/$/, ''), apiKey, schedulerName });
    navigate('/review');
  }

  const field: React.CSSProperties = { display: 'block', width: '100%', marginTop: 4, padding: '6px 8px', boxSizing: 'border-box' };

  return (
    <div style={{ maxWidth: 400, margin: '80px auto', padding: 24 }}>
      <h2 style={{ marginBottom: 24 }}>Spares</h2>
      <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
        <label>
          Server URL
          <input type="url" value={serverUrl} onChange={e => setServerUrl(e.target.value)}
            placeholder="https://your-server.example.com" required style={field} />
        </label>
        <label>
          API Key
          <input type="password" value={apiKey} onChange={e => setApiKey(e.target.value)}
            required style={field} />
        </label>
        <label>
          Scheduler
          <input type="text" value={schedulerName} onChange={e => setSchedulerName(e.target.value)}
            style={field} />
        </label>
        <button type="submit" style={{ padding: '8px 0', marginTop: 4 }}>Login</button>
      </form>
    </div>
  );
}
