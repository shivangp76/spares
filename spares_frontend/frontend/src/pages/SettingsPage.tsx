import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import { setShowReviewTimer, useShowReviewTimer } from '../preferences';
import { setThemePreference, useThemePreference, type ThemePreference } from '../theme';
import { sectionLabel } from '../utils';

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System Preference' },
];

/** Settings kept in this browser. */
export default function SettingsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const theme = useThemePreference();
  const showReviewTimer = useShowReviewTimer();

  useEffect(() => {
    if (!credentials) navigate('/login');
  }, [credentials, navigate]);

  return (
    <div style={{ maxWidth: 800, margin: '0 auto', padding: 24 }}>
      <Navbar onLogout={logout} />
      <h2 style={{ marginBottom: 16 }}>Settings</h2>

      <fieldset style={{ border: 'none', padding: 0, margin: 0 }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Theme</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
          {THEMES.map(({ value, label }) => (
            <label key={value} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="theme"
                value={value}
                checked={theme === value}
                onChange={() => setThemePreference(value)}
              />
              {label}
            </label>
          ))}
        </div>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Review</legend>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={showReviewTimer}
            onChange={e => setShowReviewTimer(e.target.checked)}
          />
          Show the running recall and rate timer
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Both durations are still recorded, and shown after each card is rated.
        </p>
      </fieldset>
    </div>
  );
}
