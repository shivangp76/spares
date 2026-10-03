import { useEffect, useRef, useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { describeReverted, redoEvent, undoEvent } from '../api/client';
import { RedoIcon, UndoIcon } from './Icons';

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
  { to: '/image-occlusion', label: 'Image Occlusion' },
  { to: '/statistics', label: 'Statistics' },
  { to: '/settings', label: 'Settings' },
];

export default function Navbar({ onLogout, extra }: NavbarProps) {
  const { pathname } = useLocation();
  const [undoStatus, setUndoStatus] = useState<string | null>(null);
  // The undo event made by this navbar's Undo, so Redo only reverses an undo the user knows about
  const [lastUndoId, setLastUndoId] = useState<number | null>(null);
  const currentLinkRef = useRef<HTMLSpanElement>(null);

  // On phones the links scroll sideways, so keep the current page's link in view
  useEffect(() => {
    currentLinkRef.current?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [pathname]);

  /** `spares event undo`: undoes the latest change of any kind, with the rest of its group. */
  async function undoLatest() {
    if (!window.confirm('Undo the most recent change? This can be any kind of change (a review, card, note or tag edit), including one made from the CLI.')) return;
    try {
      const res = await undoEvent(null);
      setUndoStatus(res ? describeReverted('Undone', res.undone_events) : 'No event to undo.');
      setLastUndoId(res?.undo_event_ids[0] ?? null);
    } catch (e) {
      setUndoStatus(String(e));
    }
  }

  /** `spares event redo`: redoes the undo made by `undoLatest`. */
  async function redoLast() {
    if (lastUndoId === null) return;
    try {
      const res = await redoEvent(lastUndoId);
      setLastUndoId(null);
      setUndoStatus(res ? describeReverted('Redone', res.redone_events) : 'No undo to redo.');
    } catch (e) {
      setUndoStatus(String(e));
    }
  }

  return (
    <nav className="navbar">
      <div className="navbar-links">
        {NAV_LINKS.map(({ to, label }) =>
          pathname === to ? (
            <span key={to} ref={currentLinkRef} className="navbar-link navbar-link-current" aria-current="page">{label}</span>
          ) : (
            <Link key={to} to={to} className="navbar-link">{label}</Link>
          )
        )}
      </div>
      <div className="navbar-actions">
        {extra}
        <button onClick={undoLatest} className="navbar-button" title="Undo the most recent change"><UndoIcon />Undo last change</button>
        {lastUndoId !== null && <button onClick={redoLast} className="navbar-button" title="Redo the change just undone"><RedoIcon />Redo last undo</button>}
        <button onClick={onLogout} className="navbar-button">Logout</button>
      </div>
      {undoStatus && (
        <div style={{ flexBasis: '100%', marginTop: 4, fontSize: 13, color: 'var(--text-secondary)', display: 'flex', gap: 8, alignItems: 'center' }}>
          <span>{undoStatus}</span>
          {/* Pages don't know what the undone or redone event changed, so offer to refetch everything */}
          <button onClick={() => window.location.reload()} style={{ padding: '2px 8px', fontSize: 12 }}>Reload page</button>
          <button onClick={() => setUndoStatus(null)} style={{ padding: '2px 8px', fontSize: 12 }}>Dismiss</button>
        </div>
      )}
    </nav>
  );
}
