import { createContext, useContext, useEffect, useRef, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { describeReverted, redoEvent, undoEvent } from "../api/client";
import { PAGE_HELP } from "../help/pages";
import type { Keybinding } from "../help/shortcuts";
import { dialogOpen, keyName, takesKeys } from "../keys";
import { NAV_LINKS, isOnPage, type NavPlace } from "../navLinks";
import NavMenu from "./NavMenu";
import { RedoIcon, UndoIcon } from "./Icons";
import OfflineIndicator from "./OfflineIndicator";
import PageHelp from "./PageHelp";
import SavedCopiesNotice from "./SavedCopiesNotice";
import SyncButton from "./SyncButton";
import UpdateIndicator from "./UpdateIndicator";
import { errorMessage } from "../utils";
import { dataChanged } from "../dataVersion";

/** The help of a page that opens it itself, with `?`, and has shortcuts of its own. */
interface Help {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  bindings: Keybinding[];
}

/** How a page's `NavbarHelp` lets the navbar's Help button open it. */
const PageHelpContext = createContext<
  (opener: { setOpen: (open: boolean) => void } | null) => void
>(() => {});

// How long after `g` its second key can be pressed
const GO_TO_MS = 1500;

/**
 * A page's help, with its shortcuts, which the navbar's Help button opens. Pages without
 * shortcuts of their own leave it out, and the navbar's `?` opens their help.
 */
export function NavbarHelp({
  help: { open, onOpenChange, bindings },
}: {
  help: Help;
}) {
  const { pathname } = useLocation();
  const setOpener = useContext(PageHelpContext);
  useEffect(() => {
    setOpener({ setOpen: onOpenChange });
    return () => setOpener(null);
  }, [setOpener, onOpenChange]);
  const pageHelp = PAGE_HELP[pathname];
  return open && pageHelp ? (
    <PageHelp
      help={pageHelp}
      bindings={bindings}
      onClose={() => onOpenChange(false)}
    />
  ) : null;
}

/** Shown above every page and kept between them, so its buttons don't flicker. */
export default function Navbar({
  onLogout,
  children,
}: {
  onLogout: () => void;
  children: React.ReactNode;
}) {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const [pageHelpOpener, setPageHelpOpener] = useState<{
    setOpen: (open: boolean) => void;
  } | null>(null);
  // The page whose help the navbar opened, so going to another closes it
  const [helpOpenOn, setHelpOpenOn] = useState<string | null>(null);
  const pageHelp = PAGE_HELP[pathname];
  const handlesHelpKey = !pageHelpOpener && !!pageHelp;

  // `?` opens the help, unless the page handles it, or it is typed into a box
  useEffect(() => {
    if (!handlesHelpKey) return;
    function handleKey(e: KeyboardEvent) {
      if (keyName(e) !== "?" || takesKeys(e.target)) return;
      e.preventDefault();
      setHelpOpenOn((open) => (open === pathname ? null : pathname));
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
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
        const link = NAV_LINKS.find((l) => l.key === name);
        if (!link) return;
        e.preventDefault();
        e.stopPropagation();
        if (link.to !== pathname) navigate(link.to);
      } else if (name === "g") {
        pressedAt = performance.now();
        e.stopPropagation();
      }
    }
    window.addEventListener("keydown", handleKey, { capture: true });
    return () =>
      window.removeEventListener("keydown", handleKey, { capture: true });
  }, [navigate, pathname]);

  // What the last undo, redo or sync did
  const [status, setStatus] = useState<string | null>(null);
  /** Shows `message`, and if the action `changed` what pages show, has them fetch it again. */
  const showStatus = (message: string, changed = false) => {
    setStatus(message);
    if (changed) dataChanged();
  };
  // The undo event made by this navbar's Undo, so Redo only reverses an undo the user knows about
  const [lastUndoId, setLastUndoId] = useState<number | null>(null);
  const currentLinkRef = useRef<HTMLSpanElement>(null);

  // On phones the links scroll sideways, so keep the current page's link in view
  useEffect(() => {
    currentLinkRef.current?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, [pathname]);

  /**
   * `spares event undo`: undoes the latest change of any kind, with the rest of its group. It isn't
   * confirmed first, as Redo is offered right after, and the status says what was undone.
   */
  async function undoLatest() {
    try {
      const res = await undoEvent(null);
      showStatus(
        res
          ? describeReverted("Undone", res.undone_events)
          : "No event to undo.",
        !!res,
      );
      setLastUndoId(res?.undo_event_ids[0] ?? null);
    } catch (e) {
      showStatus(errorMessage(e));
    }
  }

  /** `spares event redo`: redoes the undo made by `undoLatest`. */
  async function redoLast() {
    if (lastUndoId === null) return;
    try {
      const res = await redoEvent(lastUndoId);
      setLastUndoId(null);
      showStatus(
        res
          ? describeReverted("Redone", res.redone_events)
          : "No undo to redo.",
        !!res,
      );
    } catch (e) {
      showStatus(errorMessage(e));
    }
  }

  const linksIn = (place: NavPlace) =>
    NAV_LINKS.filter((l) => l.place === place);
  const currentMore = linksIn("more").find((l) => isOnPage(pathname, l.to));
  const onAccountPage = linksIn("account").some((l) =>
    isOnPage(pathname, l.to),
  );

  return (
    <PageHelpContext.Provider value={setPageHelpOpener}>
      <div className="app-navbar">
        <nav className="navbar">
          <div className="navbar-links">
            {linksIn("main").map(({ to, label }) =>
              isOnPage(pathname, to) ? (
                <span
                  key={to}
                  ref={currentLinkRef}
                  className="navbar-link navbar-link-current"
                  aria-current="page"
                >
                  {label}
                </span>
              ) : (
                <Link key={to} to={to} className="navbar-link">
                  {label}
                </Link>
              ),
            )}
            {/* The pages used less often, named after the one shown when it is one of them */}
            <span ref={currentMore ? currentLinkRef : undefined}>
              <NavMenu
                label={currentMore?.label ?? "More"}
                className="navbar-link"
                current={!!currentMore}
                title="Tags, parsers, keywords, image occlusion and the manual"
              >
                {linksIn("more").map((link) => (
                  <MenuLink key={link.to} link={link} pathname={pathname} />
                ))}
              </NavMenu>
            </span>
          </div>
          <div className="navbar-actions">
            <UpdateIndicator />
            <OfflineIndicator />
            <SavedCopiesNotice />
            <SyncButton onStatus={showStatus} />
            <span className="navbar-split">
              <button
                onClick={undoLatest}
                className="navbar-button"
                title="Undo the most recent change, of any kind, including one made from the CLI"
              >
                <UndoIcon />
                Undo
              </button>
              <NavMenu
                label={
                  <span className="visually-hidden">Redo and history</span>
                }
                title="Redo, and the history of every change"
                className="navbar-button navbar-split-end"
                align="right"
              >
                <button
                  className="nav-menu-item"
                  role="menuitem"
                  onClick={redoLast}
                  disabled={lastUndoId === null}
                >
                  <RedoIcon />
                  Redo last undo
                </button>
                {linksIn("undo").map((link) => (
                  <MenuLink key={link.to} link={link} pathname={pathname} />
                ))}
              </NavMenu>
            </span>
            {pageHelp && (
              <button
                onClick={() =>
                  pageHelpOpener
                    ? pageHelpOpener.setOpen(true)
                    : setHelpOpenOn(pathname)
                }
                className="navbar-button"
                title="What this page is for, its keyboard shortcuts, and the manual"
              >
                Help <span className="key-hint">(?)</span>
              </button>
            )}
            <NavMenu
              label="Account"
              current={onAccountPage}
              title="Settings, and logging out"
              align="right"
            >
              {linksIn("account").map((link) => (
                <MenuLink key={link.to} link={link} pathname={pathname} />
              ))}
              <button
                className="nav-menu-item"
                role="menuitem"
                onClick={onLogout}
              >
                Log out
              </button>
            </NavMenu>
          </div>
          {helpOpenOn === pathname && !pageHelpOpener && pageHelp && (
            <PageHelp help={pageHelp} onClose={() => setHelpOpenOn(null)} />
          )}
          {status && (
            <div className="navbar-status">
              <span>{status}</span>
              {lastUndoId !== null && (
                <button onClick={redoLast} className="button-small">
                  <RedoIcon />
                  Redo
                </button>
              )}
              <button onClick={() => setStatus(null)} className="button-small">
                Dismiss
              </button>
            </div>
          )}
        </nav>
      </div>
      {children}
    </PageHelpContext.Provider>
  );
}

/** A page's link in a navbar menu, marked when it is the page shown. */
function MenuLink({
  link: { to, label, key },
  pathname,
}: {
  link: { to: string; label: string; key: string };
  pathname: string;
}) {
  const current = isOnPage(pathname, to);
  return (
    <Link
      to={to}
      role="menuitem"
      className="nav-menu-item"
      aria-current={current ? "page" : undefined}
    >
      {label}
      <span className="key-hint nav-menu-key">g {key}</span>
    </Link>
  );
}
