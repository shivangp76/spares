import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { changePassword, deleteAccount, exportAccount, getCredentials } from '../api/client';
import { errorMessage, sectionLabel } from '../utils';

/** The account on the host: its password, exporting its collection, and deleting it. */
export default function AccountSettings() {
  const navigate = useNavigate();
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  let user = '';
  try {
    user = getCredentials().user;
  } catch {
    // logged out
  }

  async function run(action: () => Promise<string | null>) {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      setMessage(await action());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
      <legend style={{ ...sectionLabel, padding: 0 }}>Account{user ? ` (${user})` : ''}</legend>
      <form
        onSubmit={e => { e.preventDefault(); void run(async () => { await changePassword(current, next); setCurrent(''); setNext(''); return 'Password changed. Your other sessions have ended.'; }); }}
        style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}
      >
        <input type="password" placeholder="Current password" value={current} onChange={e => setCurrent(e.target.value)} autoComplete="current-password" required />
        <input type="password" placeholder="New password" value={next} onChange={e => setNext(e.target.value)} autoComplete="new-password" minLength={8} required />
        <button type="submit" disabled={busy}>Change password</button>
      </form>
      <div style={{ display: 'flex', gap: 8, marginTop: 12, flexWrap: 'wrap' }}>
        <button
          disabled={busy}
          onClick={() => void run(async () => {
            const blob = await exportAccount();
            const link = document.createElement('a');
            link.href = URL.createObjectURL(blob);
            link.download = `spares-${user || 'collection'}.tar.gz`;
            link.click();
            URL.revokeObjectURL(link.href);
            return 'Downloaded your collection. Another spares host imports it.';
          })}
        >
          Export my collection
        </button>
        <button
          disabled={busy}
          style={{ color: 'var(--error)' }}
          onClick={() => {
            const password = window.prompt('This deletes your account and your whole collection on this host. Type your password to delete it.');
            if (!password) return;
            void run(async () => { await deleteAccount(password); navigate('/login'); return null; });
          }}
        >
          Delete my account
        </button>
      </div>
      {message && <p role="status" style={{ marginTop: 8, color: 'var(--text-secondary)' }}>{message}</p>}
      {error && <p role="alert" style={{ marginTop: 8, color: 'var(--error)' }}>{error}</p>}
    </fieldset>
  );
}
