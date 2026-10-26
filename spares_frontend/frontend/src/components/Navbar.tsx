import { createContext, useContext, useEffect, useRef, useState } from 'react';
import { Link, useLocation, useNavigate } from 'react-router-dom';
import { describeReverted, redoEvent, undoEvent } from '../api/client';
import { PAGE_HELP } from '../help/pages';
import type { Keybinding } from '../help/shortcuts';
import { dialogOpen, keyName, takesKeys } from '../keys';
import { NAV_LINKS } from '../navLinks';
import { RedoIcon, UndoIcon } from './Icons';
import OfflineIndicator from './OfflineIndicator';
import PageHelp from './PageHelp';
import SavedCopiesNotice from './SavedCopiesNotice';
import SyncButton from './SyncButton';
import { errorMessage } from '../utils';

/** The help of a page that opens it itself, with `?`, and has shortcuts of its own. */
interface Help {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  bindings: Keybinding[];
}

/** How a page's `NavbarHelp` lets the navbar's Help button open it. */
const PageHelpContext = createContext<(opener: { setOpen: (open: boolean) => void } | null) => void>(() => {});

// How long after `g` its second key can be pressed
const GO_TO_MS = 1500;

/**
 * A page's help, with its shortcuts, which the navbar's Help button opens. Pages without
 * shortcuts of their own leave it out, and the navbar's `?` opens their help.
 */
export function NavbarHelp({ help: { open, onOpenChange, bindings } }: { help: Help }) {
  const { pathname } = useLocation();
  const setOpener = useContext(PageHelpContext);
  useEffect(() => {
    setOpener({ setOpen: onOpenChange });
    return () => setOpener(null);
  }, [setOpener, onOpenChange]);
  const pageHelp = PAGE_HELP[pathname];
  return open && pageHelp ? <PageHelp help={pageHelp} bindings={bindings} onClose={() => onOpenChange(false)} /> : null;
}

/** Shown above every page and kept between them, so its buttons don't flicker. */
export default function Navbar({ onLogout, children }: { onLogout: () => void; children: React.ReactNode }) {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const [pageHelpOpener, setPageHelpOpener] = useState<{ setOpen: (open: boolean) => void } | null>(null);
  // The page whose help the navbar opened, so going to another closes it
  const [helpOpenOn, setHelpOpenOn] = useState<string | null>(null);
  const pageHelp = PAGE_HELP[pathname];
  const handlesHelpKey = !pageHelpOpener && !!pageHelp;

  // `?` opens the help, unless the page handles it, or it is typed into a box
  useEffect(() => {
    if (!handlesHelpKey) return;
    function handleKey(e: KeyboardEvent) {
      if (keyName(e) !== '?' || takesKeys(e.target)) return;
      e.preventDefault();
      setHelpOpenOn(open => (open === pathname ? null : pathname));
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [handlesHelpKey, pathname]);
  // `g` and then a page's key in `NAV_LINKS` goes to it, as in Gmail, listed as `GO_TO_SHORTCUTS`.
  // They are listened to before the page's own keys, so that the second does nothing else, as `s`
  // would suspend the card being reviewed.
  useEffect(() => {
    let pressedAt = -Infinity;
    function handleKey(e: KeyboardEvent) {
      if (takesKeys(e.target) || dialogOpen()) return;
      const name = keyName(e);
      if (performance.now() - pressedAt < GO_TO_MS) {
        pressedAt = -Infinity;
        const link = NAV_LINKS.find(l => l.key === name);
        if (!link) return;
        e.preventDefault();
        e.stopPropagation();
        if (link.to !== pathname) navigate(link.to);
      } else if (name === 'g') {
        pressedAt = performance.now();
        e.stopPropagation();
      }
    }
    window.addEventListener('keydown', handleKey, { capture: true });
    return () => window.removeEventListener('keydown', handleKey, { capture: true });
  }, [navigate, pathname]);

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
      setUndoStatus(errorMessage(e));
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
      setUndoStatus(errorMessage(e));
    }
  }

  return (
    <PageHelpContext.Provider value={setPageHelpOpener}>
      <div className="app-navbar">
        <nav className="navbar">
          <div className="navbar-links">
            {NAV_LINKS.map(({ to, label }) =>
              pathname === to || pathname.startsWith(`${to}/`) ? (
                <span key={to} ref={currentLinkRef} className="navbar-link navbar-link-current" aria-current="page">{label}</span>
              ) : (
                <Link key={to} to={to} className="navbar-link">{label}</Link>
              )
            )}
          </div>
          <div className="navbar-actions">
            <OfflineIndicator />
            <SavedCopiesNotice />
            <SyncButton onStatus={setUndoStatus} />
            <button onClick={undoLatest} className="navbar-button" title="Undo the most recent change"><UndoIcon />Undo last change</button>
            {lastUndoId !== null && <button onClick={redoLast} className="navbar-button" title="Redo the change just undone"><RedoIcon />Redo last undo</button>}
            {pageHelp && (
              <button onClick={() => (pageHelpOpener ? pageHelpOpener.setOpen(true) : setHelpOpenOn(pathname))} className="navbar-button" title="What this page is for, its keyboard shortcuts, and the manual">
                Help <span className="key-hint">(?)</span>
              </button>
            )}
            <button onClick={onLogout} className="navbar-button">Logout</button>
          </div>
          {helpOpenOn === pathname && !pageHelpOpener && pageHelp && <PageHelp help={pageHelp} onClose={() => setHelpOpenOn(null)} />}
          {undoStatus && (
            <div style={{ flexBasis: '100%', marginTop: 4, fontSize: 13, color: 'var(--text-secondary)', display: 'flex', gap: 8, alignItems: 'center' }}>
              <span>{undoStatus}</span>
              {/* Pages don't know what the undone or redone event changed, so offer to refetch everything */}
              <button onClick={() => window.location.reload()} style={{ padding: '2px 8px', fontSize: 12 }}>Reload page</button>
              <button onClick={() => setUndoStatus(null)} style={{ padding: '2px 8px', fontSize: 12 }}>Dismiss</button>
            </div>
          )}
        </nav>
      </div>
      {children}
    </PageHelpContext.Provider>
  );
}
