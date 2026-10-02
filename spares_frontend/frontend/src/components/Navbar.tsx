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
    <nav style={{ marginBottom: 20, display: 'flex', gap: 16, alignItems: 'center', flexWrap: 'wrap' }}>
      {NAV_LINKS.map(({ to, label }) =>
        pathname === to ? (
          <span key={to} style={{ color: '#999', cursor: 'default' }}>{label}</span>
        ) : (
          <Link key={to} to={to}>{label}</Link>
        )
      )}
      <a href="/svgedit/src/editor/index.html?storagePrompt=false">Image Occlusion Editor</a>
      <div style={{ marginLeft: 'auto', display: 'flex', gap: 16, alignItems: 'center' }}>
        {extra}
        <button onClick={undoLatest} title="Undo the most recent change">Undo last change</button>
        <button onClick={onLogout}>Logout</button>
      </div>
      {undoStatus && (
        <div style={{ flexBasis: '100%', fontSize: 13, color: '#555', display: 'flex', gap: 8, alignItems: 'center' }}>
          <span>{undoStatus}</span>
          {/* Pages don't know what the undone event changed, so offer to refetch everything */}
          <button onClick={() => window.location.reload()} style={{ padding: '2px 8px', fontSize: 12 }}>Reload page</button>
          <button onClick={() => setUndoStatus(null)} style={{ padding: '2px 8px', fontSize: 12 }}>Dismiss</button>
        </div>
      )}
    </nav>
  );
}
