import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import {
  CARD_ZOOMS, PAGE_WIDTH_LABELS, PAGE_WIDTHS, setCardZoom, setPageWidth, setShowReviewTimer, setVimKeybindings, useCardZoom,
  usePageWidth, useShowReviewTimer, useVimKeybindings, type CardZoom,
} from '../preferences';
import {
  setThemePreference, setTypstThemePreference, useThemePreference, useTypstThemePreference,
  type ThemePreference, type TypstThemePreference,
} from '../theme';
import { sectionLabel } from '../utils';

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System Preference' },
];

const TYPST_THEMES: { value: TypstThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'interface', label: 'Same as Interface' },
];

/** Settings kept in this browser. */
export default function SettingsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const theme = useThemePreference();
  const typstTheme = useTypstThemePreference();
  const showReviewTimer = useShowReviewTimer();
  const vimKeybindings = useVimKeybindings();
  const pageWidth = usePageWidth();
  const cardZoom = useCardZoom();

  useEffect(() => {
    if (!credentials) navigate('/login');
  }, [credentials, navigate]);

  return (
    <div className="page">
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
        <legend style={{ ...sectionLabel, padding: 0 }}>Layout</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap', alignItems: 'center' }}>
          <span>Page width:</span>
          {PAGE_WIDTHS.map(width => (
            <label key={width} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="page-width"
                value={width}
                checked={pageWidth === width}
                onChange={() => setPageWidth(width)}
              />
              {PAGE_WIDTH_LABELS[width]}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          For large screens. Also changed with w while reviewing. Typst pages that fit the screen get longer lines, and pages of a fixed size get larger.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', marginTop: 12 }}>
          Card text size:
          <select value={cardZoom} onChange={e => setCardZoom(e.target.value as CardZoom)}>
            {CARD_ZOOMS.map(zoom => <option key={zoom} value={zoom}>{zoom}%</option>)}
          </select>
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Wherever cards are shown. Also changed with + and - while reviewing. Typst pages that fit the screen are laid out narrower and scaled up; pages of a fixed size are unchanged.
        </p>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Typst Cards</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
          {TYPST_THEMES.map(({ value, label }) => (
            <label key={value} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="typst-theme"
                value={value}
                checked={typstTheme === value}
                onChange={() => setTypstThemePreference(value)}
              />
              {label}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Also set by the Dark/Light button on each card. Dark inverts the page&apos;s colours, images included.
        </p>
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

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Editor</legend>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={vimKeybindings}
            onChange={e => setVimKeybindings(e.target.checked)}
          />
          Vim keybindings in the note editor and typed answers, and j and k to move through lists
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Off by default on touchscreens, since a phone keyboard can't leave Vim's normal mode to type.
        </p>
      </fieldset>
    </div>
  );
}
