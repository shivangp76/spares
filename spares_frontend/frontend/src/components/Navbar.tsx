import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { undoEvent } from '../api/client';

interface NavbarProps {
  onLogout: () => void;
  extra?: React.ReactNode;
}

const NAV_LINKS = [
  { to: '/review', label: 'Review' },
  { to: '/notes', label: 'Notes' },
  { to: '/cards', label: 'Cards' },
  { to: '/tags', label: 'Tags' },
  { to: '/parsers', label: 'Parsers' },
  { to: '/keywords', label: 'Keywords' },
  { to: '/statistics', label: 'Statistics' },
  { to: '/settings', label: 'Settings' },
];

export default function Navbar({ onLogout, extra }: NavbarProps) {
  const { pathname } = useLocation();
  const [undoStatus, setUndoStatus] = useState<string | null>(null);

  /** `spares event undo`: undoes the latest event of any kind, with the rest of its group. */
  async function undoLatest() {
    if (!window.confirm('Undo the most recent change? This can be any kind of change (a review, card, note or tag edit), including one made from the CLI.')) return;
    try {
      const res = await undoEvent(null);
      setUndoStatus(res ? `Undone event(s): ${res.undone_event_ids.join(', ')}.` : 'No event to undo.');
    } catch (e) {
      setUndoStatus(String(e));
    }
  }

  return (
    <nav className="navbar">
      {NAV_LINKS.map(({ to, label }) =>
        pathname === to ? (
          <span key={to} className="navbar-link navbar-link-current" aria-current="page">{label}</span>
        ) : (
          <Link key={to} to={to} className="navbar-link">{label}</Link>
        )
      )}
      <a href="/svgedit/src/editor/index.html?storagePrompt=false" className="navbar-link">Image Occlusion Editor</a>
      <div className="navbar-actions">
        {extra}
        <button onClick={undoLatest} className="navbar-button" title="Undo the most recent change">Undo last change</button>
        <button onClick={onLogout} className="navbar-button">Logout</button>
      </div>
      {undoStatus && (
        <div style={{ flexBasis: '100%', marginTop: 4, fontSize: 13, color: 'var(--text-secondary)', display: 'flex', gap: 8, alignItems: 'center' }}>
          <span>{undoStatus}</span>
          {/* Pages don't know what the undone event changed, so offer to refetch everything */}
          <button onClick={() => window.location.reload()} style={{ padding: '2px 8px', fontSize: 12 }}>Reload page</button>
          <button onClick={() => setUndoStatus(null)} style={{ padding: '2px 8px', fontSize: 12 }}>Dismiss</button>
        </div>
      )}
    </nav>
  );
}
